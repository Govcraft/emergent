"""Integration tests for publish_all and publish_stream.

These tests start a real Emergent engine, connect SDK primitives,
and verify that batched/streamed publishes are received by subscribers.
"""

import asyncio
import subprocess
import tempfile
import time
from pathlib import Path

import pytest

from emergent.handler import EmergentHandler
from emergent.sink import EmergentSink
from emergent.source import EmergentSource
from emergent.message import create_message


def _engine_binary() -> Path:
    """Locate the engine binary in the workspace target directory."""
    sdk_dir = Path(__file__).parent.parent  # sdks/py
    workspace_root = sdk_dir.parent.parent  # emergent/

    debug_bin = workspace_root / "target" / "debug" / "emergent"
    if debug_bin.exists():
        return debug_bin

    release_bin = workspace_root / "target" / "release" / "emergent"
    if release_bin.exists():
        return release_bin

    pytest.skip("engine binary not found — run 'cargo build' first")
    return Path()  # unreachable


class _TestEngine:
    """A running test engine with automatic cleanup."""

    def __init__(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self._process: subprocess.Popen[bytes] | None = None
        self.socket_path = Path(self._tmpdir.name) / "test.sock"

    async def start(self) -> None:
        config_content = f"""\
[engine]
name = "test-engine"
socket_path = "{self.socket_path}"
api_port = 0

[event_store]
json_log_dir = "{Path(self._tmpdir.name) / 'logs'}"
sqlite_path = "{Path(self._tmpdir.name) / 'events.db'}"
retention_days = 1
"""
        config_path = Path(self._tmpdir.name) / "test.toml"
        config_path.write_text(config_content)

        self._process = subprocess.Popen(
            [str(_engine_binary()), "--config", str(config_path)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        # Wait for socket to appear
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            if self.socket_path.exists():
                await asyncio.sleep(0.1)  # let IPC listener start
                return
            await asyncio.sleep(0.05)

        raise TimeoutError(f"engine socket did not appear at {self.socket_path}")

    def stop(self) -> None:
        if self._process is not None:
            self._process.kill()
            self._process.wait()
            self._process = None
        self._tmpdir.cleanup()


@pytest.fixture
async def engine():
    """Start a test engine and yield it, cleaning up on exit."""
    eng = _TestEngine()
    await eng.start()
    yield eng
    eng.stop()


@pytest.mark.asyncio
async def test_publish_all_received_by_subscriber(engine: _TestEngine) -> None:
    socket = str(engine.socket_path)

    # Subscribe first
    async with await EmergentSink.connect("test_sink", socket_path=socket) as sink:
        stream = await sink.subscribe(["test.batch"])

        # Connect source and publish batch
        async with await EmergentSource.connect("test_source", socket_path=socket) as source:
            messages = [
                create_message("test.batch").payload({"index": i})
                for i in range(5)
            ]
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
