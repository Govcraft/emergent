"""
Tests for what the client does when the engine closes the connection.

Nothing in flight can be answered once the connection is gone, so every
pending request fails at once with a ConnectionError, and the message stream
ends so an ``async for`` consumer stops.
"""

from __future__ import annotations

import asyncio
import contextlib
import socket
from typing import TYPE_CHECKING, Any

import pytest

from emergent._client import BaseClient
from emergent._protocol import MessageType
from emergent.errors import ConnectionError, EmergentError, PublishError
from emergent.message import create_message

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable

# Long enough that a request left to its timer fails the test on time alone.
TIMEOUT = 30.0
AT_ONCE = 2.0


async def reading_client() -> tuple[BaseClient, socket.socket]:
    """A client on a real socket pair with its read loop running."""
    ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    client = BaseClient(name="py80", primitive_kind="Handler", timeout=TIMEOUT)
    client._reader, client._writer = await asyncio.open_unix_connection(sock=ours)
    client._read_task = asyncio.create_task(client._read_loop())
    return client, theirs


async def until(condition: Callable[[], bool]) -> None:
    while not condition():
        await asyncio.sleep(0)


def accept_the_subscribe(client: BaseClient) -> None:
    """Answer the query's subscribe the way the engine does."""
    correlation_id = next(iter(client._pending_requests))
    client._settle_response(
        MessageType.RESPONSE, {"correlation_id": correlation_id, "success": True}
    )


def pending_queries(client: BaseClient) -> int:
    return len(client._pending_topology_requests) + len(client._pending_subscriptions_requests)


def nothing_pending(client: BaseClient) -> bool:
    return not client._pending_requests and pending_queries(client) == 0


async def failure_of(running: Awaitable[Any]) -> BaseException:
    """The error a call ends with. Waiting out the request timer fails the test."""
    with pytest.raises(EmergentError) as raised:
        await asyncio.wait_for(running, timeout=AT_ONCE)
    return raised.value


REQUESTS = [
    pytest.param(lambda c: c._discover(), id="discover"),
    pytest.param(lambda c: c._publish_ack(create_message("py80.event").build()), id="publish_ack"),
    pytest.param(lambda c: c._subscribe(["py80.event"]), id="subscribe"),
]

QUERIES = [
    pytest.param(lambda c: c._get_topology(), id="get_topology"),
    pytest.param(lambda c: c._get_my_subscriptions(), id="get_my_subscriptions"),
]


def close_after_reading(peer: socket.socket) -> None:
    """The peer has read everything sent to it, so the client reads EOF."""
    peer.setblocking(False)
    with contextlib.suppress(BlockingIOError):
        while peer.recv(65536):
            pass
    peer.close()


def close_without_reading(peer: socket.socket) -> None:
    """Unread bytes at close reset the connection, so the client's read fails."""
    peer.close()


ENDINGS = [
    pytest.param(close_after_reading, id="eof"),
    pytest.param(close_without_reading, id="read-error"),
]


class TestARequestInFlight:
    @pytest.mark.parametrize("ending", ENDINGS)
    @pytest.mark.parametrize("request_", REQUESTS)
    async def test_it_fails_at_once_with_a_connection_error(
        self,
        request_: Callable[[BaseClient], Awaitable[Any]],
        ending: Callable[[socket.socket], Any],
    ) -> None:
        client, peer = await reading_client()
        running = asyncio.ensure_future(request_(client))
        await until(lambda: bool(client._pending_requests))

        ending(peer)
        error = await failure_of(running)

        assert type(error) is ConnectionError
        assert str(error) == "Connection closed"
        assert nothing_pending(client)
        client.close()

    @pytest.mark.parametrize("ending", ENDINGS)
    @pytest.mark.parametrize("query", QUERIES)
    async def test_a_query_fails_at_once_with_a_connection_error(
        self,
        query: Callable[[BaseClient], Awaitable[Any]],
        ending: Callable[[socket.socket], Any],
    ) -> None:
        client, peer = await reading_client()
        running = asyncio.ensure_future(query(client))
        await until(lambda: bool(client._pending_requests))
        accept_the_subscribe(client)
        await until(lambda: pending_queries(client) == 1)

        ending(peer)
        error = await failure_of(running)

        assert type(error) is ConnectionError
        assert str(error) == "Connection closed"
        assert nothing_pending(client)
        client.close()


# The socket refuses the write, so these are the errors of a refused write.
LATER_REQUESTS = [
    pytest.param(lambda c: c._discover(), ConnectionError, id="discover"),
    pytest.param(
        lambda c: c._publish_ack(create_message("py80.event").build()),
        PublishError,
        id="publish_ack",
    ),
    pytest.param(lambda c: c._get_topology(), ConnectionError, id="get_topology"),
]


class TestARequestAfterTheLoss:
    @pytest.mark.parametrize("ending", ENDINGS)
    @pytest.mark.parametrize(("request_", "error_type"), LATER_REQUESTS)
    async def test_it_fails_at_once_too(
        self,
        request_: Callable[[BaseClient], Awaitable[Any]],
        error_type: type[EmergentError],
        ending: Callable[[socket.socket], Any],
    ) -> None:
        client, peer = await reading_client()
        ending(peer)
        await until(lambda: client._read_task is not None and client._read_task.done())

        error = await failure_of(request_(client))

        assert type(error) is error_type
        assert isinstance(error.__cause__, OSError)
        assert nothing_pending(client)
        client.close()


class TestTheMessageStream:
    @pytest.mark.parametrize("ending", ENDINGS)
    async def test_an_async_for_consumer_stops(
        self, ending: Callable[[socket.socket], Any]
    ) -> None:
        client, peer = await reading_client()
        subscribing = asyncio.ensure_future(client._subscribe(["py80.event"]))
        await until(lambda: bool(client._pending_requests))
        accept_the_subscribe(client)
        stream = await subscribing

        async def consume() -> int:
            return len([message async for message in stream])

        consuming = asyncio.ensure_future(consume())
        await asyncio.sleep(0)
        ending(peer)

        assert await asyncio.wait_for(consuming, timeout=AT_ONCE) == 0
        assert stream.closed
        assert client._message_stream is None
        client.close()


class TestClose:
    async def test_it_fails_what_is_in_flight_the_same_way(self) -> None:
        client, peer = await reading_client()
        running = asyncio.ensure_future(client._discover())
        await until(lambda: bool(client._pending_requests))

        client.close()
        error = await failure_of(running)

        assert type(error) is ConnectionError
        assert str(error) == "Connection closed"
        assert nothing_pending(client)
        peer.close()
