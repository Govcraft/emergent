"""
Convenience functions for building Emergent primitives.

These helpers eliminate the boilerplate code for connecting, handling signals,
and running the event loop. Developers only need to provide their business logic
as an async function.

Example (Source with custom logic - interval-based timer):
    >>> from emergent.helpers import run_source
    >>> from emergent import create_message
    >>> import asyncio
    >>>
    >>> async def timer_logic(source, shutdown_event):
    ...     count = 0
    ...     while not shutdown_event.is_set():
    ...         try:
    ...             await asyncio.wait_for(shutdown_event.wait(), timeout=3.0)
    ...             break  # Shutdown requested
    ...         except asyncio.TimeoutError:
    ...             count += 1
    ...             await source.publish(
    ...                 create_message("timer.tick").payload({"count": count})
    ...             )
    >>>
    >>> await run_source("my_timer", timer_logic)

Example (Source as one-shot function):
    >>> from emergent.helpers import run_source
    >>> from emergent import create_message
    >>>
    >>> async def init_logic(source, shutdown_event):
    ...     await source.publish(create_message("system.init"))
    ...     # Function completes, source disconnects automatically
    >>>
    >>> await run_source("init", init_logic)

Example (Handler with message transformation):
    >>> from emergent.helpers import run_handler
    >>> from emergent import create_message
    >>>
    >>> async def on_message(msg, handler):
    ...     await handler.publish(
    ...         create_message("timer.processed")
    ...         .caused_by(msg.id)
    ...         .payload({"processed": True})
    ...     )
    >>>
    >>> await run_handler("my_handler", ["timer.tick"], on_message)

Example (Sink with message consumption):
    >>> from emergent.helpers import run_sink
    >>>
    >>> async def on_message(msg):
    ...     print(f"Received: {msg.payload}")
    >>>
    >>> await run_sink("my_sink", ["timer.processed"], on_message)
"""

from __future__ import annotations

import asyncio
import os
import signal
from typing import TYPE_CHECKING, Callable

from emergent.handler import EmergentHandler
from emergent.sink import EmergentSink
from emergent.source import EmergentSource

if TYPE_CHECKING:
    from emergent.types import EmergentMessage

# Default environment variable name for the primitive name.
EMERGENT_NAME_ENV = "EMERGENT_NAME"


class HelperError(Exception):
    """Errors that can occur in helper functions."""

    pass


# Type aliases for callbacks
SourceRunFn = Callable[[EmergentSource, asyncio.Event], "asyncio.Future[None]"]
HandlerProcessFn = Callable[["EmergentMessage", EmergentHandler], "asyncio.Future[None]"]
SinkConsumeFn = Callable[["EmergentMessage"], "asyncio.Future[None]"]


def _resolve_name(name: str | None, default: str) -> str:
    """Resolve the primitive name from the provided option or environment variable."""
    if name is not None:
        return name
    env_name = os.environ.get(EMERGENT_NAME_ENV)
    return env_name if env_name is not None else default


async def run_source(
    name: str | None,
    run_fn: Callable[[EmergentSource, asyncio.Event], "asyncio.Future[None]"],
) -> None:
    """
    Run a Source with custom logic.

    This function handles all the boilerplate for running a Source:
    - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
    - Connects to the Emergent engine
    - Sets up SIGTERM/SIGINT signal handling for graceful shutdown
    - Calls your function with the connected source and a shutdown event
    - Gracefully disconnects after your function completes

    Your function receives:
    - `source: EmergentSource` - The connected source for publishing messages
    - `shutdown_event: asyncio.Event` - An event that is set when shutdown is requested

    Args:
        name: Optional name for this source. Falls back to `EMERGENT_NAME` env var,
            then to "source".
        run_fn: Async function that implements your source logic.

    Raises:
        HelperError: If connection fails or user function raises an exception.

    Example (interval-based timer):
        >>> async def timer_logic(source, shutdown_event):
        ...     count = 0
        ...     while not shutdown_event.is_set():
        ...         try:
        ...             await asyncio.wait_for(shutdown_event.wait(), timeout=3.0)
        ...             break
        ...         except asyncio.TimeoutError:
        ...             count += 1
        ...             await source.publish(create_message("timer.tick").payload({"count": count}))
        >>>
        >>> await run_source("my_timer", timer_logic)

    Example (one-shot source):
        >>> async def init_logic(source, shutdown_event):
        ...     await source.publish(create_message("system.init"))
        >>>
        >>> await run_source("init", init_logic)
    """
    resolved_name = _resolve_name(name, "source")

    try:
        source = await EmergentSource.connect(resolved_name)
    except Exception as e:
        raise HelperError(
            f"failed to connect to Emergent engine as '{resolved_name}': {e}"
        ) from e

    # Set up signal handlers for graceful shutdown
    loop = asyncio.get_running_loop()
    shutdown_event = asyncio.Event()

    def signal_handler() -> None:
        shutdown_event.set()

    # Register signal handlers
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, signal_handler)

    try:
        await run_fn(source, shutdown_event)
    except HelperError:
        raise
    except Exception as e:
        raise HelperError(f"user function error: {e}") from e
    finally:
        # Clean up signal handlers
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.remove_signal_handler(sig)
        # Graceful disconnect
        await source.disconnect()


async def run_handler(
    name: str | None,
    subscriptions: list[str],
    process_fn: Callable[["EmergentMessage", EmergentHandler], "asyncio.Future[None]"],
) -> None:
    """
    Run a Handler with message processing.

    This function handles all the boilerplate for running a Handler:
    - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
    - Connects to the Emergent engine
    - Subscribes to the specified message types
    - Sets up SIGTERM/SIGINT signal handling for graceful shutdown
    - Runs the message loop, calling your function for each message
    - Gracefully disconnects on shutdown

    Args:
        name: Optional name for this handler. Falls back to `EMERGENT_NAME` env var,
            then to "handler".
        subscriptions: Message types to subscribe to.
        process_fn: Async function called for each message with (msg, handler).

    Raises:
        HelperError: If connection, subscription fails, or user function raises.

    Example:
        >>> from emergent.helpers import run_handler
        >>> from emergent import create_message
        >>>
        >>> async def on_message(msg, handler):
        ...     await handler.publish(
        ...         create_message("timer.processed")
        ...         .caused_by(msg.id)
        ...         .payload({"processed": True})
        ...     )
        >>>
        >>> await run_handler("my_handler", ["timer.tick"], on_message)
    """
    resolved_name = _resolve_name(name, "handler")

    try:
        handler = await EmergentHandler.connect(resolved_name)
    except Exception as e:
        raise HelperError(
            f"failed to connect to Emergent engine as '{resolved_name}': {e}"
        ) from e

    try:
        stream = await handler.subscribe(subscriptions)
    except Exception as e:
        handler.close()
        raise HelperError(f"failed to subscribe: {e}") from e

    running = True

    # Set up signal handlers for graceful shutdown
    loop = asyncio.get_running_loop()

    def signal_handler() -> None:
        nonlocal running
        running = False
        stream.close()

    # Register signal handlers
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, signal_handler)

    try:
        async for msg in stream:
            if not running:
                break
            try:
                await process_fn(msg, handler)
            except Exception as e:
                raise HelperError(f"user function error: {e}") from e
    finally:
        # Clean up signal handlers
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.remove_signal_handler(sig)
        # Close stream and disconnect
        stream.close()
        await handler.disconnect()


async def run_sink(
    name: str | None,
    subscriptions: list[str],
    consume_fn: Callable[["EmergentMessage"], "asyncio.Future[None]"],
) -> None:
    """
    Run a Sink with message consumption.

    This function handles all the boilerplate for running a Sink:
    - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
    - Connects to the Emergent engine
    - Subscribes to the specified message types
    - Sets up SIGTERM/SIGINT signal handling for graceful shutdown
    - Runs the message loop, calling your function for each message
    - Gracefully disconnects on shutdown

    Args:
        name: Optional name for this sink. Falls back to `EMERGENT_NAME` env var,
            then to "sink".
        subscriptions: Message types to subscribe to.
        consume_fn: Async function called for each message with (msg).

    Raises:
        HelperError: If connection, subscription fails, or user function raises.

    Example:
        >>> from emergent.helpers import run_sink
        >>>
        >>> async def on_message(msg):
        ...     print(f"Received: {msg.payload}")
        >>>
        >>> await run_sink("my_sink", ["timer.processed"], on_message)
    """
    resolved_name = _resolve_name(name, "sink")

    try:
        sink = await EmergentSink.connect(resolved_name)
    except Exception as e:
        raise HelperError(
            f"failed to connect to Emergent engine as '{resolved_name}': {e}"
        ) from e

    try:
        stream = await sink.subscribe(subscriptions)
    except Exception as e:
        sink.close()
        raise HelperError(f"failed to subscribe: {e}") from e

    running = True

    # Set up signal handlers for graceful shutdown
    loop = asyncio.get_running_loop()

    def signal_handler() -> None:
        nonlocal running
        running = False
        stream.close()

    # Register signal handlers
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, signal_handler)

    try:
        async for msg in stream:
            if not running:
                break
            try:
                await consume_fn(msg)
            except Exception as e:
                raise HelperError(f"user function error: {e}") from e
    finally:
        # Clean up signal handlers
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.remove_signal_handler(sig)
        # Close stream and disconnect
        stream.close()
        await sink.disconnect()
