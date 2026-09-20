"""
Tests for what run_source does when the engine closes the connection.

A Source subscribes to nothing, so no stream ends to tell it the engine is
gone. The shutdown event is the only signal its function waits on, so a lost
connection has to set it, the way SIGTERM does.
"""

from __future__ import annotations

import asyncio
import socket
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from emergent import EmergentSource, create_message
from emergent.helpers import run_source

if TYPE_CHECKING:
    from collections.abc import Callable, Iterator

AT_ONCE = 2.0


@pytest.fixture
def listener(monkeypatch: pytest.MonkeyPatch) -> Iterator[socket.socket]:
    """A listening Unix socket standing in for the engine's."""
    with tempfile.TemporaryDirectory(prefix="py83-") as directory:
        path = Path(directory) / "engine.sock"
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(str(path))
        server.listen(1)
        server.setblocking(False)
        monkeypatch.setenv("EMERGENT_SOCKET", str(path))
        yield server
        server.close()


def close_after_reading(peer: socket.socket) -> None:
    """The peer takes everything sent and closes: the client reads EOF."""
    peer.setblocking(False)
    try:
        while peer.recv(65536):
            pass
    except BlockingIOError:
        pass
    peer.close()


def close_without_reading(peer: socket.socket) -> None:
    """The peer closes with bytes unread: the client's read is reset."""
    peer.close()


ENDINGS = [close_after_reading, close_without_reading]


@pytest.mark.parametrize("end", ENDINGS)
async def test_a_lost_connection_sets_the_shutdown_event(
    listener: socket.socket, end: Callable[[socket.socket], None]
) -> None:
    published = asyncio.Event()

    async def wait_for_shutdown(source: EmergentSource, shutdown: asyncio.Event) -> None:
        await source.publish(create_message("py83.event"))
        published.set()
        await shutdown.wait()

    loop = asyncio.get_running_loop()
    running = asyncio.ensure_future(run_source("py83", wait_for_shutdown))
    peer, _ = await loop.sock_accept(listener)
    await published.wait()

    end(peer)

    await asyncio.wait_for(running, timeout=AT_ONCE)


async def test_the_function_returning_first_is_not_disturbed(
    listener: socket.socket,
) -> None:
    seen: list[bool] = []

    async def one_shot(source: EmergentSource, shutdown: asyncio.Event) -> None:
        await source.publish(create_message("py83.event"))
        seen.append(shutdown.is_set())

    loop = asyncio.get_running_loop()
    running = asyncio.ensure_future(run_source("py83", one_shot))
    peer, _ = await loop.sock_accept(listener)

    await asyncio.wait_for(running, timeout=AT_ONCE)

    assert seen == [False]
    peer.close()


async def test_close_is_not_a_lost_connection(listener: socket.socket) -> None:
    told: list[str] = []
    source = await EmergentSource.connect("py83")
    peer, _ = await asyncio.get_running_loop().sock_accept(listener)
    source._when_connection_lost(lambda: told.append("lost"))

    source.close()
    await asyncio.sleep(0.05)

    assert told == []
    peer.close()


async def test_a_connection_already_lost_is_reported_at_once(
    listener: socket.socket,
) -> None:
    told: list[str] = []
    source = await EmergentSource.connect("py83")
    peer, _ = await asyncio.get_running_loop().sock_accept(listener)
    peer.close()
    read_task = source._read_task
    assert read_task is not None
    await asyncio.wait_for(read_task, timeout=AT_ONCE)

    source._when_connection_lost(lambda: told.append("lost"))

    assert told == ["lost"]
    source.close()
