"""
EmergentSource - Publish-only client primitive.

A Source can only publish messages (ingress).
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from ._client import BaseClient
from ._protocol import Format
from .message import MessageBuilder, create_message

if TYPE_CHECKING:
    from .types import DiscoveryInfo, EmergentMessage


class EmergentSource(BaseClient):
    """
    A Source can only publish messages.

    Use this for ingress components that bring data into the Emergent system,
    such as HTTP endpoints, sensors, or external API integrations.

    Example:
        >>> # Simple usage with context manager
        >>> async with await EmergentSource.connect("my_source") as source:
        ...     # Shorthand publish: type + payload
        ...     await source.publish("sensor.reading", {"value": 42.5, "unit": "celsius"})
        ...
        ...     # Full control with builder
        ...     await source.publish(
        ...         create_message("sensor.reading")
        ...         .payload({"value": 42.5, "unit": "celsius"})
        ...         .metadata({"sensor_id": "temp-01"})
        ...     )
    """

    def __init__(
        self,
        name: str,
        *,
        timeout: float = 30.0,
        format_: Format = Format.JSON,
    ) -> None:
        """
        Create a new EmergentSource.

        Use `EmergentSource.connect()` instead of calling this directly.

        Args:
            name: Unique name for this source
            timeout: Request timeout in seconds
            format_: Serialization format
        """
        super().__init__(
            name=name,
            primitive_kind="Source",
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
        format_: Format = Format.JSON,
    ) -> EmergentSource:
        """
        Connect to the Emergent engine as a Source.

        Args:
            name: Unique name for this source
            socket_path: Custom socket path (overrides EMERGENT_SOCKET env var)
            timeout: Request timeout in seconds
            format_: Serialization format

        Returns:
            A connected EmergentSource instance

        Example:
            >>> source = await EmergentSource.connect("my_source")
            >>> # ... use source ...
            >>> source.close()

            >>> # Or with automatic cleanup:
            >>> async with await EmergentSource.connect("my_source") as source:
            ...     await source.publish("timer.tick", {"count": 1})
        """
        source = cls(name, timeout=timeout, format_=format_)
        await source._connect(socket_path)
        return source

    async def publish(
        self,
        message_or_type: EmergentMessage | MessageBuilder | str,
        payload: Any = None,
    ) -> None:
        """
        Publish a message.

        Supports multiple calling patterns for maximum ergonomics:

        Args:
            message_or_type: Either a message type string, MessageBuilder, or
                             complete EmergentMessage
            payload: Optional payload when using shorthand (type string)

        Example:
            >>> # 1. Shorthand: type + payload (most common)
            >>> await source.publish("timer.tick", {"count": 1})

            >>> # 2. MessageBuilder (auto-calls .build())
            >>> await source.publish(
            ...     create_message("timer.tick")
            ...     .payload({"count": 1})
            ...     .metadata({"trace_id": "abc"})
            ... )

            >>> # 3. Complete EmergentMessage
            >>> await source.publish(message)
        """
        message: EmergentMessage

        if isinstance(message_or_type, str):
            # Shorthand: publish("type", { payload })
            message = create_message(message_or_type).payload(payload).build()
        elif isinstance(message_or_type, MessageBuilder):
            # MessageBuilder: auto-call build()
            message = message_or_type.build()
        else:
            # Already an EmergentMessage
            message = message_or_type

        await self._publish(message)

    async def discover(self) -> DiscoveryInfo:
        """
        Discover available message types and primitives.

        Returns:
            Discovery information from the engine

        Example:
            >>> info = await source.discover()
            >>> print("Available types:", info.message_types)
            >>> print("Connected primitives:", info.primitives)
        """
        return await self._discover()

    async def __aenter__(self) -> EmergentSource:
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
