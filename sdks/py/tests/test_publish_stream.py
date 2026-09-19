"""Integration tests for publish_all and publish_stream.

These tests start a real Emergent engine, connect SDK primitives,
and verify that batched/streamed publishes are received by subscribers.
"""

import asyncio

import pytest

from emergent.message import create_message
from emergent.sink import EmergentSink
from emergent.source import EmergentSource

from .conftest import _TestEngine


@pytest.mark.asyncio
async def test_publish_all_received_by_subscriber(engine: _TestEngine) -> None:
    socket = str(engine.socket_path)

    # Subscribe first
    async with await EmergentSink.connect("test_sink", socket_path=socket) as sink:
        stream = await sink.subscribe(["test.batch"])

        # Connect source and publish batch
        async with await EmergentSource.connect("test_source", socket_path=socket) as source:
            messages = [create_message("test.batch").payload({"index": i}) for i in range(5)]
            count = await source.publish_all(messages)

        assert count == 5

        # Collect messages with timeout
        received = []

        async def collect() -> None:
            async for msg in stream:
                received.append(msg)
                if len(received) >= 5:
                    break

        await asyncio.wait_for(collect(), timeout=5.0)
        assert len(received) == 5

        for i, msg in enumerate(received):
            assert msg.payload["index"] == i


@pytest.mark.asyncio
async def test_publish_stream_received_by_subscriber(engine: _TestEngine) -> None:
    socket = str(engine.socket_path)

    async with await EmergentSink.connect("test_sink", socket_path=socket) as sink:
        stream = await sink.subscribe(["test.stream"])

        async with await EmergentSource.connect("test_source", socket_path=socket) as source:

            async def generate_messages():
                for i in range(3):
                    yield create_message("test.stream").payload({"seq": i})

            count = await source.publish_stream(generate_messages())

        assert count == 3

        received = []

        async def collect() -> None:
            async for msg in stream:
                received.append(msg)
                if len(received) >= 3:
                    break

        await asyncio.wait_for(collect(), timeout=5.0)
        assert len(received) == 3

        for i, msg in enumerate(received):
            assert msg.payload["seq"] == i


@pytest.mark.asyncio
async def test_publish_all_empty_iterable(engine: _TestEngine) -> None:
    socket = str(engine.socket_path)

    async with await EmergentSource.connect("test_source", socket_path=socket) as source:
        count = await source.publish_all([])

    assert count == 0
