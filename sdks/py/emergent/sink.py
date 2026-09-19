"""
EmergentSink - Subscribe-only client primitive.

A Sink can only subscribe to messages (egress).
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from ._client import BaseClient
from ._protocol import DEFAULT_FORMAT, Format

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from .stream import MessageStream
    from .types import DiscoveryInfo, EmergentMessage, TopologyState


def needs_configured_topics(requested: list[str]) -> bool:
    """
    Report whether the engine must be asked for the configured subscribe list.

    `EmergentSink.messages` only falls back to the engine's configuration when
    the caller requested no topics of their own.

    Args:
        requested: Topics the caller asked for

    Returns:
        True when the configured list is needed
    """
    return not requested


def resolve_topics(requested: list[str], configured: list[str]) -> list[str]:
    """
    Resolve which topics to subscribe to.

    Explicitly requested topics always win. The configured list is the fallback
    for callers that pass nothing, which keeps the engine's TOML the source of
    truth for sinks that do not hard-code their own subscriptions.

    Args:
        requested: Topics the caller asked for
        configured: Topics the engine has configured for this primitive

    Returns:
        The topics to subscribe to
    """
    return requested if requested else configured


class EmergentSink(BaseClient):
    """
    A Sink can only subscribe to messages (no publishing).

    Use this for egress components that send data out of the Emergent system,
    such as database writers, notification senders, or external API clients.

    Example (one-liner):
        >>> # Simplest: one-liner consumption (recommended for simple cases)
        >>> async for msg in EmergentSink.messages("my_sink", ["timer.tick"]):
        ...     print(msg.payload)

    Example (explicit):
        >>> # Standard: explicit lifecycle with automatic cleanup
        >>> async with await EmergentSink.connect("my_sink") as sink:
        ...     async with await sink.subscribe(["timer.tick", "timer.filtered"]) as stream:
        ...         async for msg in stream:
        ...             data = msg.payload_as(dict)
        ...             print(f"Tick {data['count']}")
    """

    def __init__(
        self,
        name: str,
        *,
        timeout: float = 30.0,
        format_: Format = DEFAULT_FORMAT,
    ) -> None:
        """
        Create a new EmergentSink.

        Use `EmergentSink.connect()` instead of calling this directly.

        Args:
            name: Unique name for this sink
            timeout: Request timeout in seconds
            format_: Serialization format
        """
        super().__init__(
            name=name,
            primitive_kind="Sink",
            timeout=timeout,
            format=format_,
        )

    @classmethod
    async def connect(
        cls,
        name: str,
        *,
        socket_path: str | None = None,
        timeout: float = 30.0,
        format_: Format = DEFAULT_FORMAT,
    ) -> EmergentSink:
        """
        Connect to the Emergent engine as a Sink.

        Args:
            name: Unique name for this sink
            socket_path: Custom socket path (overrides EMERGENT_SOCKET env var)
            timeout: Request timeout in seconds
            format_: Serialization format

        Returns:
            A connected EmergentSink instance

        Example:
            >>> sink = await EmergentSink.connect("my_sink")
            >>> # ... use sink ...
            >>> sink.close()

            >>> # Or with automatic cleanup:
            >>> async with await EmergentSink.connect("my_sink") as sink:
            ...     pass
        """
        sink = cls(name, timeout=timeout, format_=format_)
        await sink._connect(socket_path)
        return sink

    @classmethod
    async def messages(
        cls,
        name: str,
        types: list[str],
        *,
        socket_path: str | None = None,
        timeout: float = 30.0,
        format_: Format = DEFAULT_FORMAT,
    ) -> AsyncIterator[EmergentMessage]:
        """
        Convenience method for one-liner message consumption.

        Connects, subscribes, and yields messages. Automatically cleans up
        when the iteration completes or breaks.

        The `types` you pass win. Pass an empty list to defer to the engine
        instead: the sink then queries its configured `subscribes` list from the
        engine's TOML and subscribes to that. The engine is only consulted when
        `types` is empty.

        Args:
            name: Unique name for this sink
            types: Message types to subscribe to; empty falls back to the
                engine's configured subscriptions
            socket_path: Custom socket path (overrides EMERGENT_SOCKET env var)
            timeout: Request timeout in seconds
            format_: Serialization format

        Yields:
            EmergentMessage instances

        Example:
            >>> # Minimal boilerplate consumption
            >>> async for msg in EmergentSink.messages("console_sink", ["timer.tick"]):
            ...     print(f"[{msg.message_type}]", msg.payload)
            ...     if should_stop():
            ...         break  # Breaking auto-cleans up

            >>> # With options
            >>> async for msg in EmergentSink.messages("my_sink", ["event.*"], timeout=60.0):
            ...     process_message(msg)

            >>> # Defer to the engine's configured `subscribes` list
            >>> async for msg in EmergentSink.messages("my_sink", []):
            ...     process_message(msg)
        """
        sink = await cls.connect(
            name,
            socket_path=socket_path,
            timeout=timeout,
            format_=format_,
        )

        try:
            # Only ask the engine when the caller requested nothing.
            configured_types = (
                await sink.get_my_subscriptions() if needs_configured_topics(types) else []
            )
            topics = resolve_topics(types, configured_types)

            stream = await sink.subscribe(topics)
            try:
                async for msg in stream:
                    yield msg
            finally:
                stream.close()
        finally:
            sink.close()

    async def subscribe(
        self,
        types_or_first: list[str] | str,
        *rest: str,
    ) -> MessageStream:
        """
        Subscribe to message types and receive them via MessageStream.

        Supports both array and variadic arguments for convenience.

        Args:
            types_or_first: Either a list of types or the first type
            rest: Additional types when using variadic style

        Returns:
            A MessageStream for receiving messages

        Example:
            >>> # Array style
            >>> stream = await sink.subscribe(["timer.tick", "timer.filtered"])

            >>> # Variadic style
            >>> stream = await sink.subscribe("timer.tick", "timer.filtered")

            >>> async for msg in stream:
            ...     print(msg.message_type, msg.payload)
        """
        types = types_or_first if isinstance(types_or_first, list) else [types_or_first, *rest]

        return await self._subscribe(types)

    async def unsubscribe(self, message_types: list[str]) -> None:
        """
        Unsubscribe from message types.

        Args:
            message_types: List of message types to unsubscribe from

        Example:
            >>> await sink.unsubscribe(["timer.tick"])
        """
        await self._unsubscribe(message_types)

    async def discover(self) -> DiscoveryInfo:
        """
        Discover available message types and primitives.

        Returns:
            Discovery information from the engine

        Example:
            >>> info = await sink.discover()
            >>> print("Available types:", info.message_types)
            >>> print("Connected primitives:", info.primitives)
        """
        return await self._discover()

    async def get_my_subscriptions(self) -> list[str]:
        """
        Get the configured subscription types for this primitive.

        Queries the engine's config service to get the message types
        this sink should subscribe to based on the engine configuration.

        Returns:
            List of message type names to subscribe to

        Example:
            >>> sink = await EmergentSink.connect("my_sink")
            >>> types = await sink.get_my_subscriptions()
            >>> print("Configured to receive:", types)
        """
        return await self._get_my_subscriptions()

    async def get_topology(self) -> TopologyState:
        """
        Get the current topology (all primitives and their state).

        Queries the engine to get the current state of all registered
        primitives, including their publish/subscribe configuration.

        Returns:
            TopologyState with all primitives and their current state

        Example:
            >>> sink = await EmergentSink.connect("my_sink")
            >>> topology = await sink.get_topology()
            >>> for prim in topology.primitives:
            ...     print(f"{prim.name} ({prim.kind}): {prim.state}")
        """
        return await self._get_topology()

    async def __aenter__(self) -> EmergentSink:
        """Enter the async context manager."""
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: object,
    ) -> None:
        """Exit the async context manager and disconnect."""
        await self.disconnect()

    # Note: Sinks do NOT have a publish() method - they are consume-only
