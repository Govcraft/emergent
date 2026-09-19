"""Pytest configuration and fixtures."""

import asyncio
import subprocess
import tempfile
import time
from pathlib import Path

import pytest

# Configure pytest-asyncio
pytest_plugins = ["pytest_asyncio"]


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

    pytest.skip("engine binary not found, run 'cargo build' first")
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
json_log_dir = "{Path(self._tmpdir.name) / "logs"}"
sqlite_path = "{Path(self._tmpdir.name) / "events.db"}"
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
