"""
Tests for which message stream the client feeds and closes.

The client keeps one registered stream. A stream nobody holds any more must
not stay registered, and closing a stream that was already replaced must not
unregister the one that replaced it.
"""

from __future__ import annotations

import asyncio
import socket
from typing import TYPE_CHECKING, Any

import pytest

from emergent._client import BaseClient
from emergent._protocol import MessageType, encode_frame
from emergent.errors import ConnectionError, EmergentError, TimeoutError

if TYPE_CHECKING:
    from collections.abc import Callable

    from emergent.stream import MessageStream


async def connected_client(timeout: float) -> tuple[BaseClient, socket.socket]:
    ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    client = BaseClient(name="py82", primitive_kind="Sink", timeout=timeout)
    client._reader, client._writer = await asyncio.open_unix_connection(sock=ours)
    return client, theirs


async def until(condition: Callable[[], bool]) -> None:
    while not condition():
        await asyncio.sleep(0)


async def subscribed(client: BaseClient, message_type: str) -> MessageStream:
    """Subscribe with the engine's acceptance fed straight to the client."""
    subscribing = asyncio.ensure_future(client._subscribe([message_type]))
    await until(lambda: bool(client._pending_requests))
    correlation_id = next(iter(client._pending_requests))
    client._settle_response(
        MessageType.RESPONSE, {"correlation_id": correlation_id, "success": True}
    )
    return await subscribing


def push(client: BaseClient, message_type: str, payload: dict[str, Any]) -> None:
    """Deliver one message the way the engine's PUSH frame does."""
    notification = {
        "notification_id": "ntf_01",
        "message_type": message_type,
        "timestamp_ms": 1700000000001,
        "payload": {
            "id": "msg_01",
            "message_type": message_type,
            "source": "upstream",
            "timestamp_ms": 1700000000000,
            "payload": payload,
        },
    }
    client._read_buffer.extend(encode_frame(MessageType.PUSH, notification))
    client._process_frames()


class TestAReplacedStream:
    async def test_closing_it_leaves_the_current_stream_registered(self) -> None:
        client, peer = await connected_client(timeout=5.0)
        first = await subscribed(client, "py82.first")
        second = await subscribed(client, "py82.second")

        first.close()

        assert client._message_stream is second
        push(client, "py82.second", {"n": 1})
        message = await asyncio.wait_for(second.next(), timeout=1.0)
        assert message is not None
        assert message.payload == {"n": 1}
        client.close()
        peer.close()

    async def test_the_current_stream_still_ends_on_close(self) -> None:
        client, peer = await connected_client(timeout=5.0)
        first = await subscribed(client, "py82.first")
        second = await subscribed(client, "py82.second")
        first.close()

        async def consume() -> int:
            return len([message async for message in second])

        consuming = asyncio.ensure_future(consume())
        await asyncio.sleep(0)
        client.close()

        assert await asyncio.wait_for(consuming, timeout=1.0) == 0
        assert second.closed
        peer.close()

    async def test_closing_the_current_stream_unregisters_it(self) -> None:
        client, peer = await connected_client(timeout=5.0)
        stream = await subscribed(client, "py82.only")

        stream.close()

        assert client._message_stream is None
        client.close()
        peer.close()


class TestASecondSubscribe:
    async def test_it_ends_the_stream_it_replaces(self) -> None:
        client, peer = await connected_client(timeout=5.0)
        first = await subscribed(client, "py86.first")

        async def consume() -> int:
            return len([message async for message in first])

        consuming = asyncio.ensure_future(consume())
        await asyncio.sleep(0)

        second = await subscribed(client, "py86.second")

        assert await asyncio.wait_for(consuming, timeout=1.0) == 0
        assert first.closed
        assert client._message_stream is second
        assert not second.closed
        client.close()
        peer.close()

    async def test_the_new_stream_gets_what_is_pushed_afterwards(self) -> None:
        client, peer = await connected_client(timeout=5.0)
        await subscribed(client, "py86.first")
        second = await subscribed(client, "py86.second")

        # The engine keeps the first subscription, so its topic still arrives.
        push(client, "py86.first", {"n": 1})

        message = await asyncio.wait_for(second.next(), timeout=1.0)
        assert message is not None
        assert message.payload == {"n": 1}
        client.close()
        peer.close()

    async def test_it_ends_the_earlier_stream_even_when_it_then_fails(self) -> None:
        client, peer = await connected_client(timeout=0.05)
        first = await subscribed(client, "py86.first")

        with pytest.raises(TimeoutError):
            await client._subscribe(["py86.second"])

        assert first.closed
        assert client._message_stream is None
        client.close()
        peer.close()


class TestASubscribeThatRaises:
    async def test_a_timed_out_subscribe_closes_and_unregisters_its_stream(self) -> None:
        client, peer = await connected_client(timeout=0.05)
        subscribing = asyncio.ensure_future(client._subscribe(["py82.event"]))
        await until(lambda: bool(client._pending_requests))
        stream = client._message_stream
        assert stream is not None

        with pytest.raises(TimeoutError):
            await subscribing

        assert stream.closed
        assert client._message_stream is None
        client.close()
        peer.close()

    async def test_a_refused_subscribe_closes_and_unregisters_its_stream(self) -> None:
        client, peer = await connected_client(timeout=5.0)
        peer.close()

        with pytest.raises(ConnectionError):
            await client._subscribe(["py82.event"])

        assert client._message_stream is None
        client.close()

    async def test_a_timed_out_pattern_subscribe_does_the_same(self) -> None:
        client, peer = await connected_client(timeout=0.05)
        subscribing = asyncio.ensure_future(client._subscribe(["py82.event", "py82.*"]))
        await until(lambda: bool(client._pending_requests))
        stream = client._message_stream
        assert stream is not None
        correlation_id = next(iter(client._pending_requests))
        client._settle_response(
            MessageType.RESPONSE, {"correlation_id": correlation_id, "success": True}
        )

        with pytest.raises(EmergentError):
            await subscribing

        assert stream.closed
        assert client._message_stream is None
        client.close()
        peer.close()
