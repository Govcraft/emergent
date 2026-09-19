"""
Tests for writing frames to a socket that takes only part of a buffer.

`StreamWriter.write` never writes part of a buffer and gives up: what the
socket does not take at once the transport keeps and sends later, in the order
`write` was called. These tests pin that with a real socket whose send buffer
is as small as the kernel allows.
"""

from __future__ import annotations

import asyncio
import socket
from typing import TYPE_CHECKING

from emergent._client import BaseClient
from emergent._protocol import DecodedFrame, next_frame
from emergent.message import create_message

if TYPE_CHECKING:
    from collections.abc import Callable

    from emergent.types import EmergentMessage

PUBLISHERS = 6
PER_PUBLISHER = 4


def padded(index: int) -> EmergentMessage:
    """A message far larger than the send buffer. Sizes differ so that a frame
    cut short cannot line up with the next one by accident."""
    return create_message("py69.event").payload({"pad": "z" * (200_000 + 1000 * index)}).build()


def published_id(frame: DecodedFrame) -> str:
    """The message ID inside a publish envelope."""
    message_id = frame.payload["payload"]["inner"]["id"]
    assert isinstance(message_id, str)
    return message_id


async def read_frames(
    reader: asyncio.StreamReader, count: int, backlog: Callable[[], int], seen: list[int]
) -> list[str]:
    """Read slowly and in small pieces so the writers wait part way through.

    `seen` collects how many bytes the writing side still held back each time.
    """
    buffer = bytearray()
    ids: list[str] = []
    while len(ids) < count:
        seen.append(backlog())
        chunk = await reader.read(8192)
        assert chunk, f"socket closed after {len(ids)} frames"
        buffer.extend(chunk)
        await asyncio.sleep(0)
        while True:
            step = next_frame(bytes(buffer))
            if step is None:
                break
            assert isinstance(step, DecodedFrame), f"frame {len(ids) + 1} on the wire: {step}"
            ids.append(published_id(step))
            del buffer[: step.bytes_consumed]
    assert not buffer, f"{len(buffer)} stray bytes after the last frame"
    return ids


async def test_concurrent_publishes_stay_whole_on_a_slow_socket() -> None:
    ours, theirs = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    ours.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 1)
    client = BaseClient(name="py69", primitive_kind="Handler", timeout=5.0)
    client._reader, client._writer = await asyncio.open_unix_connection(sock=ours)
    server_reader, server_writer = await asyncio.open_unix_connection(sock=theirs)

    async def publisher(start: int) -> list[str]:
        sent = []
        for index in range(start, start + PER_PUBLISHER):
            message = padded(index)
            sent.append(message.id)
            await client._publish(message)
        return sent

    total = PUBLISHERS * PER_PUBLISHER
    held_back: list[int] = []
    backlog = client._writer.transport.get_write_buffer_size
    try:
        received, *sent = await asyncio.wait_for(
            asyncio.gather(
                read_frames(server_reader, total, backlog, held_back),
                *(publisher(p * PER_PUBLISHER) for p in range(PUBLISHERS)),
            ),
            timeout=30,
        )
    finally:
        server_writer.close()
        client._writer.close()

    # The socket did refuse part of a frame, or this test proves nothing.
    assert max(held_back) > 0

    assert sorted(received) == sorted(i for batch in sent for i in batch)
    # One publisher's frames keep the order it sent them in.
    for batch in sent:
        assert [i for i in received if i in batch] == batch
