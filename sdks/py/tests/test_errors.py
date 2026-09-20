"""Tests for the error classes and the failure paths that raise them.

Through SDK 0.13.1 ``SubscriptionError`` and ``DiscoveryError`` were exported
and never raised: those failures raised a plain ``ConnectionError``. Both are
``ConnectionError`` subclasses, so an ``except ConnectionError`` written
against the old behavior still catches them.
"""

import asyncio
from collections.abc import Awaitable, Callable
from typing import Any

import pytest

from emergent._client import BaseClient
from emergent._protocol import MessageType, try_decode_frame
from emergent.errors import (
    ConnectionError,
    DiscoveryError,
    DisposedError,
    EmergentError,
    ProtocolError,
    PublishError,
    SocketNotFoundError,
    StreamError,
    SubscriptionError,
    TimeoutError,
    ValidationError,
)
from emergent.message import create_message


class TestTheClassHierarchy:
    @pytest.mark.parametrize(
        ("error", "code"),
        [
            (ConnectionError("x"), "CONNECTION_FAILED"),
            (SubscriptionError("x"), "SUBSCRIPTION_FAILED"),
            (DiscoveryError("x"), "DISCOVERY_FAILED"),
            (PublishError("x"), "PUBLISH_FAILED"),
            (SocketNotFoundError("/s"), "SOCKET_NOT_FOUND"),
            (TimeoutError("x", 5.0), "TIMEOUT"),
            (ProtocolError("x"), "PROTOCOL_ERROR"),
            (DisposedError("Sink"), "DISPOSED"),
            (StreamError("x"), "STREAM_ERROR"),
            (ValidationError("x", "f"), "VALIDATION_ERROR"),
        ],
    )
    def test_each_error_carries_its_own_code(self, error: EmergentError, code: str) -> None:
        assert error.code == code
        assert isinstance(error, EmergentError)

    @pytest.mark.parametrize("error", [SubscriptionError("x", ["a.b"]), DiscoveryError("x")])
    def test_the_engine_rejection_errors_are_connection_errors(self, error: EmergentError) -> None:
        assert isinstance(error, ConnectionError)
        assert str(error) == "x"

    @pytest.mark.parametrize(
        "error",
        [
            PublishError("x"),
            SocketNotFoundError("/s"),
            TimeoutError(),
            ProtocolError("x"),
            DisposedError("Sink"),
            StreamError("x"),
            ValidationError("x", "f"),
        ],
    )
    def test_the_other_errors_are_not_connection_errors(self, error: EmergentError) -> None:
        assert not isinstance(error, ConnectionError)

    def test_a_subscription_error_keeps_what_was_rejected(self) -> None:
        assert SubscriptionError("x", ["a.b"]).message_types == ["a.b"]
        assert SubscriptionError("x").message_types == []


class FakeWriter:
    """Stands in for the socket writer and keeps what the client sent."""

    def __init__(self) -> None:
        self.frames: list[bytes] = []

    def write(self, data: bytes) -> None:
        self.frames.append(data)

    async def drain(self) -> None:
        return None


class Rejecting:
    """A client with no socket whose requests the test answers."""

    def __init__(self) -> None:
        self.client = BaseClient(name="py68", primitive_kind="Handler", timeout=5.0)
        self.writer = FakeWriter()
        self.client._writer = self.writer  # type: ignore[assignment]
        self.answered = 0

    async def next_request(self) -> dict[str, Any]:
        """Wait for the next request the client writes and return its body."""
        for _ in range(100):
            if len(self.writer.frames) > self.answered:
                break
            await asyncio.sleep(0)
        else:
            raise AssertionError("the client never wrote a frame")

        decoded = try_decode_frame(self.writer.frames[self.answered])
        assert decoded is not None
        self.answered += 1
        body: dict[str, Any] = decoded.payload
        return body

    async def answer(self, success: bool, error: str | None = None) -> None:
        """Answer the next request the client writes, as the engine does."""
        request = await self.next_request()
        # The engine picks the frame type from the success flag.
        self.client._handle_frame(
            MessageType.RESPONSE if success else MessageType.ERROR,
            {"correlation_id": request["correlation_id"], "success": success, "error": error},
        )


class TestTheFailurePaths:
    async def test_a_rejected_subscription_raises_a_subscription_error(self) -> None:
        probe = Rejecting()
        task = asyncio.create_task(probe.client._subscribe(["py68.event", "py68.other.*"]))
        await probe.answer(False, "Subscription limit reached")

        with pytest.raises(SubscriptionError, match="Subscription limit reached") as raised:
            await asyncio.wait_for(task, timeout=1.0)
        assert isinstance(raised.value, ConnectionError)
        assert raised.value.code == "SUBSCRIPTION_FAILED"
        assert raised.value.message_types == ["py68.event"]

    async def test_a_rejected_pattern_subscription_names_the_patterns(self) -> None:
        probe = Rejecting()
        task = asyncio.create_task(probe.client._subscribe(["py68.event", "py68.other.*"]))
        await probe.answer(True)
        await probe.answer(False, "Pattern limit reached")

        with pytest.raises(SubscriptionError, match="Pattern limit reached") as raised:
            await asyncio.wait_for(task, timeout=1.0)
        assert isinstance(raised.value, ConnectionError)
        assert raised.value.message_types == ["py68.other.*"]

    async def test_a_rejected_discovery_raises_a_discovery_error(self) -> None:
        probe = Rejecting()
        task = asyncio.create_task(probe.client._discover())
        await probe.answer(False, "Parse error: bad discover request")

        with pytest.raises(DiscoveryError, match="Parse error: bad discover request") as raised:
            await asyncio.wait_for(task, timeout=1.0)
        assert isinstance(raised.value, ConnectionError)
        assert raised.value.code == "DISCOVERY_FAILED"

    async def test_a_malformed_discovery_reply_raises_a_discovery_error(self) -> None:
        probe = Rejecting()
        task = asyncio.create_task(probe.client._discover())
        request = await probe.next_request()
        probe.client._handle_frame(
            MessageType.RESPONSE,
            {
                "correlation_id": request["correlation_id"],
                "success": True,
                "message_types": "not a list",
            },
        )

        with pytest.raises(DiscoveryError, match="Malformed discovery response") as raised:
            await asyncio.wait_for(task, timeout=1.0)
        assert isinstance(raised.value, ConnectionError)

    async def test_a_rejected_acknowledged_publish_raises_a_publish_error(self) -> None:
        probe = Rejecting()
        message = create_message("py68.event").build()
        task = asyncio.create_task(probe.client._publish_ack(message))
        await probe.answer(False, "Actor not found: message_broker")

        with pytest.raises(PublishError, match="Actor not found: message_broker") as raised:
            await asyncio.wait_for(task, timeout=1.0)
        assert raised.value.message_type == "py68.event"

    @pytest.mark.parametrize(
        ("call", "reply_type"),
        [
            (lambda client: client._get_topology(), "system.response.topology"),
            (lambda client: client._get_my_subscriptions(), "system.response.subscriptions"),
        ],
    )
    async def test_a_query_whose_reply_subscription_is_rejected_says_which(
        self, call: Callable[[BaseClient], Awaitable[Any]], reply_type: str
    ) -> None:
        probe = Rejecting()
        task = asyncio.ensure_future(call(probe.client))
        await probe.answer(False)

        with pytest.raises(SubscriptionError) as raised:
            await asyncio.wait_for(task, timeout=1.0)
        assert isinstance(raised.value, ConnectionError)
        assert raised.value.message_types == [reply_type]
