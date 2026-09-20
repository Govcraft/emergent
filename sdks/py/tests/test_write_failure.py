"""
Tests for what the client raises when the socket refuses a write.

Once the engine has gone away the operating system refuses the next write.
The caller sees an SDK error for it, with the OS error as ``__cause__``.
"""

from __future__ import annotations

import asyncio
import socket
from typing import TYPE_CHECKING, Any

import pytest

from emergent._client import BaseClient, write_failure
from emergent.errors import ConnectionError, EmergentError, PublishError
from emergent.message import create_message

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable

OS_ERRORS = [
    pytest.param(BrokenPipeError(32, "Broken pipe"), id="broken-pipe"),
    pytest.param(ConnectionResetError("Connection lost"), id="connection-reset"),
    pytest.param(OSError(9, "Bad file descriptor"), id="bad-descriptor"),
]


class TestWriteFailure:
    @pytest.mark.parametrize("cause", OS_ERRORS)
    def test_a_refused_publish_is_a_publish_error(self, cause: OSError) -> None:
        error = write_failure(cause, "py77.event")

        assert isinstance(error, PublishError)
        assert error.code == "PUBLISH_FAILED"
        assert error.message_type == "py77.event"
        assert str(error) == f"Failed to publish: {cause}"

    @pytest.mark.parametrize("cause", OS_ERRORS)
    def test_a_refused_request_is_a_connection_error(self, cause: OSError) -> None:
        error = write_failure(cause, None)

        assert isinstance(error, ConnectionError)
        assert not isinstance(error, PublishError)
        assert error.code == "CONNECTION_FAILED"
        assert str(error) == f"Failed to send: {cause}"

    def test_an_empty_message_type_is_still_a_publish(self) -> None:
        assert isinstance(write_failure(BrokenPipeError(), ""), PublishError)


async def client_with_closed_peer() -> BaseClient:
    ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    client = BaseClient(name="py77", primitive_kind="Handler", timeout=1.0)
    client._reader, client._writer = await asyncio.open_unix_connection(sock=ours)
    theirs.close()
    return client


OPERATIONS = [
    pytest.param(
        lambda c: c._publish(create_message("py77.event").build()),
        PublishError,
        "PUBLISH_FAILED",
        id="publish",
    ),
    pytest.param(
        lambda c: c._publish_ack(create_message("py77.event").build()),
        PublishError,
        "PUBLISH_FAILED",
        id="publish_ack",
    ),
    pytest.param(
        lambda c: c._subscribe(["py77.event"]),
        ConnectionError,
        "CONNECTION_FAILED",
        id="subscribe",
    ),
    pytest.param(
        lambda c: c._subscribe(["py77.*"]),
        ConnectionError,
        "CONNECTION_FAILED",
        id="pattern-subscribe",
    ),
    pytest.param(
        lambda c: c._unsubscribe(["py77.event"]),
        ConnectionError,
        "CONNECTION_FAILED",
        id="unsubscribe",
    ),
    pytest.param(lambda c: c._discover(), ConnectionError, "CONNECTION_FAILED", id="discover"),
    pytest.param(
        lambda c: c._get_topology(), ConnectionError, "CONNECTION_FAILED", id="get_topology"
    ),
    pytest.param(
        lambda c: c._get_my_subscriptions(),
        ConnectionError,
        "CONNECTION_FAILED",
        id="get_my_subscriptions",
    ),
]


class TestAClosedPeer:
    @pytest.mark.parametrize(("operation", "error_type", "code"), OPERATIONS)
    async def test_the_operation_raises_an_sdk_error_that_keeps_the_os_error(
        self,
        operation: Callable[[BaseClient], Awaitable[Any]],
        error_type: type[EmergentError],
        code: str,
    ) -> None:
        client = await client_with_closed_peer()

        with pytest.raises(EmergentError) as raised:
            await operation(client)

        assert type(raised.value) is error_type
        assert raised.value.code == code
        assert isinstance(raised.value.__cause__, OSError)
        assert str(raised.value.__cause__) in str(raised.value)
        assert client._pending_requests == {}

    async def test_a_refused_publish_names_its_message_type(self) -> None:
        client = await client_with_closed_peer()

        with pytest.raises(PublishError) as raised:
            await client._publish(create_message("py77.named").build())

        assert raised.value.message_type == "py77.named"
