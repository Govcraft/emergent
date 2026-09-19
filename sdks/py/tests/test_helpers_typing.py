"""Type-level test for the helper callback signatures.

The helpers await whatever a callback returns, and every caller passes an
``async def`` function, which returns a coroutine. The callback types were once
written as ``Callable[..., asyncio.Future[None]]``, which a coroutine function
does not satisfy, so strict mypy rejected the documented usage.
"""

from pathlib import Path

import pytest

CALLER = """\
import asyncio
from collections.abc import Awaitable

from emergent import EmergentHandler, EmergentMessage, EmergentSource
from emergent.helpers import run_handler, run_sink, run_source


async def tick(source: EmergentSource, shutdown: asyncio.Event) -> None:
    await shutdown.wait()


async def process(msg: EmergentMessage, handler: EmergentHandler) -> None:
    return None


async def consume(msg: EmergentMessage) -> None:
    return None


def consume_with_a_future(msg: EmergentMessage) -> Awaitable[None]:
    future: asyncio.Future[None] = asyncio.get_running_loop().create_future()
    future.set_result(None)
    return future


async def main() -> None:
    await run_source("s", tick)
    await run_handler("h", ["a.b"], process)
    await run_sink("k", ["a.b"], consume)
    await run_sink("k", ["a.b"], consume_with_a_future)
"""

NOT_AWAITABLE = """\
from emergent import EmergentMessage
from emergent.helpers import run_sink


def consume(msg: EmergentMessage) -> None:
    return None


async def main() -> None:
    await run_sink("k", ["a.b"], consume)
"""


def run_mypy(tmp_path: Path, source: str) -> tuple[str, int]:
    api = pytest.importorskip("mypy.api")
    caller = tmp_path / "caller.py"
    caller.write_text(source)
    config = Path(__file__).parent.parent / "pyproject.toml"
    stdout, stderr, status = api.run(["--config-file", str(config), str(caller)])
    return stdout + stderr, status


def test_async_def_callbacks_type_check(tmp_path: Path) -> None:
    output, status = run_mypy(tmp_path, CALLER)

    assert status == 0, output


def test_a_plain_function_is_still_rejected(tmp_path: Path) -> None:
    output, status = run_mypy(tmp_path, NOT_AWAITABLE)

    assert status == 1, output
    assert "run_sink" in output
    assert "arg-type" in output
