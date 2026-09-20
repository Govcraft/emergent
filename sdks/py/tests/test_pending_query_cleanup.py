"""
Tests for what a topology or subscriptions query leaves behind when it fails.

Both queries subscribe to their response type, register a pending entry with
a timeout timer, then publish the request. Whatever ends the query, the entry
and the timer go with it.
"""

from __future__ import annotations

import asyncio
import socket
from typing import TYPE_CHECKING, Any

import pytest

from emergent._client import BaseClient, PendingTopologyRequest, discard_pending
from emergent._protocol import MessageType
from emergent.errors import PublishError

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable

QUERIES = [
    pytest.param(lambda c: c._get_topology(), id="get_topology"),
    pytest.param(lambda c: c._get_my_subscriptions(), id="get_my_subscriptions"),
]


class TimerLog:
    """Records every timer the client arms, through the loop's own call_later."""

    def __init__(self, loop: asyncio.AbstractEventLoop, monkeypatch: pytest.MonkeyPatch) -> None:
        self.handles: list[asyncio.TimerHandle] = []
        call_later = loop.call_later

        def recording(delay: float, callback: Any, *args: Any) -> asyncio.TimerHandle:
            handle = call_later(delay, callback, *args)
            self.handles.append(handle)
            return handle

        monkeypatch.setattr(loop, "call_later", recording)

    def armed(self) -> list[asyncio.TimerHandle]:
        return [handle for handle in self.handles if not handle.cancelled()]


async def connected_client() -> tuple[BaseClient, socket.socket]:
    ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    client = BaseClient(name="py79", primitive_kind="Handler", timeout=30.0)
    client._reader, client._writer = await asyncio.open_unix_connection(sock=ours)
    return client, theirs


async def the_subscribe_is_sent(client: BaseClient) -> None:
    while not client._pending_requests:
        await asyncio.sleep(0)


async def accept_the_subscribe(client: BaseClient) -> None:
    """Answer the query's subscribe the way the engine does."""
    await the_subscribe_is_sent(client)
    correlation_id = next(iter(client._pending_requests))
    client._settle_response(
        MessageType.RESPONSE, {"correlation_id": correlation_id, "success": True}
    )


def pending_queries(client: BaseClient) -> int:
    return len(client._pending_topology_requests) + len(client._pending_subscriptions_requests)


class TestDiscardPending:
    def test_it_removes_the_entry_and_cancels_its_timer(self) -> None:
        loop = asyncio.new_event_loop()
        try:
            timer = loop.call_later(30.0, lambda: None)
            pending = {"cor_1": PendingTopologyRequest(future=loop.create_future(), timer=timer)}

            discard_pending(pending, "cor_1")

            assert pending == {}
            assert timer.cancelled()
        finally:
            loop.close()

    def test_an_entry_without_a_timer_is_removed(self) -> None:
        loop = asyncio.new_event_loop()
        try:
            pending = {"cor_1": PendingTopologyRequest(future=loop.create_future())}

            discard_pending(pending, "cor_1")

            assert pending == {}
        finally:
            loop.close()

    def test_an_entry_already_gone_is_not_an_error(self) -> None:
        pending: dict[str, PendingTopologyRequest] = {}

        discard_pending(pending, "cor_1")

        assert pending == {}


class TestAQueryThatFails:
    @pytest.mark.parametrize("query", QUERIES)
    async def test_a_refused_request_publish_leaves_nothing_pending(
        self,
        query: Callable[[BaseClient], Awaitable[Any]],
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        timers = TimerLog(asyncio.get_running_loop(), monkeypatch)
        client, peer = await connected_client()
        running = asyncio.ensure_future(query(client))

        # The engine takes the subscribe and is gone before the request.
        await the_subscribe_is_sent(client)
        peer.close()
        await accept_the_subscribe(client)

        with pytest.raises(PublishError) as raised:
            await running

        assert isinstance(raised.value.__cause__, OSError)
        assert pending_queries(client) == 0
        assert client._pending_requests == {}
        assert timers.armed() == []

    @pytest.mark.parametrize("query", QUERIES)
    async def test_a_cancelled_query_leaves_nothing_pending(
        self,
        query: Callable[[BaseClient], Awaitable[Any]],
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        timers = TimerLog(asyncio.get_running_loop(), monkeypatch)
        client, peer = await connected_client()
        running = asyncio.ensure_future(query(client))
        await accept_the_subscribe(client)
        while pending_queries(client) == 0:
            await asyncio.sleep(0)

        running.cancel()
        with pytest.raises(asyncio.CancelledError):
            await running

        assert pending_queries(client) == 0
        assert timers.armed() == []
        peer.close()
