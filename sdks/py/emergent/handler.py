"""
EmergentHandler - Subscribe and publish client primitive.

A Handler can both subscribe to and publish messages (bidirectional).
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from ._client import BaseClient
from ._protocol import Format
from .message import MessageBuilder, create_message

if TYPE_CHECKING:
    from .stream import MessageStream
    from .types import DiscoveryInfo, EmergentMessage


class EmergentHandler(BaseClient):
    """
    A Handler can both subscribe to and publish messages.

    Use this for processing components that transform, enrich, or route data.
    Handlers are the workhorses of Emergent systems.

    Example:
        >>> async with await EmergentHandler.connect("order_processor") as handler:
        ...     async with await handler.subscribe(["order.created"]) as stream:
        ...         async for msg in stream:
        ...             order = msg.payload_as(Order)
        ...
        ...             # Process and publish result with causation tracking
        ...             await handler.publish(
        ...                 create_message("order.processed")
        ...                 .caused_by(msg.id)
        ...                 .payload({"order_id": order.id, "status": "ok"})
        ...             )
    """

    def __init__(
        self,
        name: str,
        *,
        timeout: float = 30.0,
        format_: Format = Format.JSON,
    ) -> None:
        """
        Create a new EmergentHandler.

        Use `EmergentHandler.connect()` instead of calling this directly.

        Args:
            name: Unique name for this handler
            timeout: Request timeout in seconds
            format_: Serialization format
        """
        super().__init__(
            name=name,
            primitive_kind="Handler",
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
    ) -> EmergentHandler:
        """
        Connect to the Emergent engine as a Handler.

        Args:
            name: Unique name for this handler
            socket_path: Custom socket path (overrides EMERGENT_SOCKET env var)
            timeout: Request timeout in seconds
            format_: Serialization format

        Returns:
            A connected EmergentHandler instance

        Example:
            >>> handler = await EmergentHandler.connect("my_handler")
            >>> # ... use handler ...
            >>> handler.close()

            >>> # Or with automatic cleanup:
            >>> async with await EmergentHandler.connect("my_handler") as handler:
            ...     pass
        """
        handler = cls(name, timeout=timeout, format_=format_)
        await handler._connect(socket_path)
        return handler

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
            >>> stream = await handler.subscribe(["order.created", "order.updated"])

            >>> # Variadic style
            >>> stream = await handler.subscribe("order.created", "order.updated")

            >>> for await msg in stream:
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
            >>> await handler.unsubscribe(["order.created"])
        """
        await self._unsubscribe(message_types)

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
            >>> await handler.publish("order.processed", {"status": "ok"})

            >>> # 2. With causation tracking (recommended for handlers)
            >>> await handler.publish(
            ...     create_message("order.processed")
            ...     .caused_by(original_msg.id)
            ...     .payload({"status": "ok"})
            ... )

            >>> # 3. MessageBuilder (auto-calls .build())
            >>> await handler.publish(
            ...     create_message("order.processed")
            ...     .caused_by(original_msg.id)
            ...     .payload({"status": "ok"})
            ... )

            >>> # 4. Complete EmergentMessage
            >>> await handler.publish(message)
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
            >>> info = await handler.discover()
            >>> print("Available types:", info.message_types)
        """
        return await self._discover()

    async def __aenter__(self) -> EmergentHandler:
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
