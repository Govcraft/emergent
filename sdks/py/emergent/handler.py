"""
EmergentHandler - Subscribe and publish client primitive.

A Handler can both subscribe to and publish messages (bidirectional).
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterable, AsyncIterator, Iterable
from typing import TYPE_CHECKING, Any

from ._client import BaseClient
from ._protocol import Format, generate_correlation_id
from .message import MessageBuilder, create_message

if TYPE_CHECKING:
    from .stream import MessageStream
    from .types import DiscoveryInfo, EmergentMessage

_STREAM_READY = "stream.ready"
_STREAM_PULL = "stream.pull"
_STREAM_END = "stream.end"


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

    async def publish_ack(
        self,
        message_or_type: EmergentMessage | MessageBuilder | str,
        payload: Any = None,
    ) -> None:
        """
        Publish a message with broker acknowledgment (backpressure).

        Unlike :meth:`publish`, this waits for the engine's message broker to
        confirm it has processed and forwarded the message before returning.

        Used internally by :meth:`publish_all` and :meth:`publish_stream`.
        """
        message: EmergentMessage

        if isinstance(message_or_type, str):
            message = create_message(message_or_type).payload(payload).build()
        elif isinstance(message_or_type, MessageBuilder):
            message = message_or_type.build()
        else:
            message = message_or_type

        await self._publish_ack(message)

    async def publish_all(
        self,
        messages: Iterable[EmergentMessage | MessageBuilder],
    ) -> int:
        """
        Publish all messages from an iterable.

        Sends each message individually so subscribers begin consuming
        immediately. Stops on the first error.

        Args:
            messages: Iterable of messages to publish

        Returns:
            The number of messages successfully published.

        Example:
            >>> messages = [
            ...     create_message("record.processed")
            ...     .caused_by(original_msg.id)
            ...     .payload(record)
            ...     for record in records
            ... ]
            >>> count = await handler.publish_all(messages)
        """
        count = 0
        for message in messages:
            await self.publish_ack(message)
            count += 1
        return count

    async def publish_stream(
        self,
        messages: AsyncIterable[EmergentMessage | MessageBuilder],
    ) -> int:
        """
        Publish messages from an async iterable (stream).

        Consumes the async iterable, publishing each message individually so
        subscribers begin consuming immediately. Stops on the first publish
        error or when the iterable ends.

        Args:
            messages: Async iterable of messages to publish

        Returns:
            The number of messages successfully published.

        Example:
            >>> async def generate_messages():
            ...     for i in range(100):
            ...         yield create_message("batch.item").payload({"index": i})
            >>> count = await handler.publish_stream(generate_messages())
        """
        count = 0
        async for message in messages:
            await self.publish_ack(message)
            count += 1
        return count

    async def stream_offer(
        self,
        message_type: str,
        items: Iterable[Any] | AsyncIterable[Any],
        pull_stream: MessageStream,
        *,
        timeout: float = 30.0,
    ) -> int:
        """
        Offer items as a pull-based stream with consumer-driven backpressure.

        Publishes ``stream.ready``, then serves items one at a time as the
        consumer sends ``stream.pull`` requests. Publishes ``stream.end``
        when exhausted. Non-stream messages that arrive during the offer
        are buffered and replayed after.

        Args:
            message_type: Message type for data items (e.g. "classify.result")
            items: Iterable or async iterable of payload values to stream
            pull_stream: The MessageStream to read pull requests from
            timeout: Seconds to wait for each pull request

        Returns:
            Number of items published
        """
        from .errors import StreamError

        stream_id = generate_correlation_id("strm")
        buffered: list[EmergentMessage] = []

        # Determine count if possible
        count_hint: int | None = None
        if hasattr(items, "__len__"):
            count_hint = len(items)  # type: ignore[arg-type]

        # Create iterator — handle both sync and async iterables
        if isinstance(items, AsyncIterable):
            items_aiter = items.__aiter__()
            is_async = True
        else:
            items_iter = iter(items)
            is_async = False

        def _next_sync() -> tuple[Any, bool]:
            try:
                return next(items_iter), True  # type: ignore[arg-type]
            except StopIteration:
                return None, False

        async def _next_async() -> tuple[Any, bool]:
            try:
                return await items_aiter.__anext__(), True  # type: ignore[union-attr]
            except StopAsyncIteration:
                return None, False

        try:
            # Announce stream
            await self.publish(
                create_message(_STREAM_READY).payload(
                    {"stream_id": stream_id, "message_type": message_type, "count": count_hint}
                )
            )

            published = 0

            while True:
                # Wait for pull request
                try:
                    msg = await asyncio.wait_for(pull_stream.next(), timeout=timeout)
                except asyncio.TimeoutError:
                    raise StreamError(
                        f"Timed out waiting for stream.pull after {timeout}s",
                        stream_id=stream_id,
                    ) from None

                if msg is None:
                    raise StreamError(
                        "Pull stream closed during stream_offer",
                        stream_id=stream_id,
                    )

                # Check if it's a pull request for this stream
                if (
                    msg.message_type == _STREAM_PULL
                    and isinstance(msg.payload, dict)
                    and msg.payload.get("stream_id") == stream_id
                ):
                    # Get next item
                    if is_async:
                        item, has_item = await _next_async()
                    else:
                        item, has_item = _next_sync()

                    if has_item:
                        await self.publish(
                            create_message(message_type)
                                .payload(item)
                                .metadata({"stream_id": stream_id})
                        )
                        published += 1
                    else:
                        # Stream exhausted
                        await self.publish(
                            create_message(_STREAM_END).payload({"stream_id": stream_id})
                        )
                        break
                else:
                    # Not a pull for this stream — buffer it
                    buffered.append(msg)

            return published
        finally:
            # Replay buffered messages
            for msg in buffered:
                pull_stream.push(msg)

    async def stream_consume(
        self,
        message_type: str,
        source_stream: MessageStream,
        *,
        timeout: float = 30.0,
    ) -> AsyncIterator[EmergentMessage]:
        """
        Consume a pull-based stream, yielding items one at a time.

        Waits for ``stream.ready``, then sends ``stream.pull`` requests
        automatically after each item is consumed. Iteration stops on
        ``stream.end``.

        Args:
            message_type: Message type to consume (e.g. "classify.result")
            source_stream: The MessageStream to read items from
            timeout: Seconds to wait for each item

        Yields:
            EmergentMessage instances containing stream items
        """
        from .errors import StreamError

        buffered: list[EmergentMessage] = []

        try:
            # Wait for stream.ready
            stream_id: str | None = None
            while stream_id is None:
                try:
                    msg = await asyncio.wait_for(source_stream.next(), timeout=timeout)
                except asyncio.TimeoutError:
                    raise StreamError(
                        f"Timed out waiting for stream.ready after {timeout}s",
                    ) from None

                if msg is None:
                    raise StreamError("Source stream closed before stream.ready")

                if (
                    msg.message_type == _STREAM_READY
                    and isinstance(msg.payload, dict)
                    and msg.payload.get("message_type") == message_type
                ):
                    stream_id = msg.payload["stream_id"]
                else:
                    buffered.append(msg)

            # Send initial pull
            await self.publish(
                create_message(_STREAM_PULL).payload({"stream_id": stream_id})
            )

            # Yield items until stream.end
            while True:
                try:
                    msg = await asyncio.wait_for(source_stream.next(), timeout=timeout)
                except asyncio.TimeoutError:
                    raise StreamError(
                        f"Timed out waiting for stream item after {timeout}s",
                        stream_id=stream_id,
                    ) from None

                if msg is None:
                    raise StreamError(
                        "Source stream closed during stream_consume",
                        stream_id=stream_id,
                    )

                if (
                    msg.message_type == _STREAM_END
                    and isinstance(msg.payload, dict)
                    and msg.payload.get("stream_id") == stream_id
                ):
                    break

                if (
                    msg.message_type == message_type
                    and isinstance(msg.metadata, dict)
                    and msg.metadata.get("stream_id") == stream_id
                ):
                    yield msg
                    # Request next item
                    await self.publish(
                        create_message(_STREAM_PULL).payload({"stream_id": stream_id})
                    )
                else:
                    buffered.append(msg)
        finally:
            for msg in buffered:
                source_stream.push(msg)

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
