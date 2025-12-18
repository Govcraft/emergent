"""
MessageStream for consuming messages from subscriptions.

This module provides an async iterator for receiving messages.
"""

from __future__ import annotations

import asyncio
from collections import deque
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Callable

    from .types import EmergentMessage


class MessageStream:
    """
    Async iterator for receiving messages from subscriptions.

    Implements `AsyncIterator` for use with `async for` loops.
    Also implements async context manager for automatic cleanup.

    Attributes:
        closed: Whether the stream has been closed
        pending: Number of messages waiting in the queue

    Example:
        >>> async with await sink.subscribe(["timer.tick"]) as stream:
        ...     async for msg in stream:
        ...         print(msg.message_type, msg.payload)
        ...         if should_stop():
        ...             break

    Example (explicit):
        >>> stream = await sink.subscribe(["timer.tick"])
        >>> try:
        ...     async for msg in stream:
        ...         print(msg.payload)
        ... finally:
        ...     stream.close()
    """

    def __init__(
        self,
        *,
        on_close: Callable[[], None] | None = None,
        max_queue_size: int = 1000,
    ) -> None:
        """
        Create a new MessageStream.

        Args:
            on_close: Optional callback when stream closes
            max_queue_size: Maximum queue size before oldest messages are dropped
        """
        self._queue: deque[EmergentMessage] = deque(maxlen=max_queue_size)
        self._waiting: asyncio.Future[EmergentMessage | None] | None = None
        self._closed = False
        self._on_close = on_close

    @property
    def closed(self) -> bool:
        """Check if the stream is closed."""
        return self._closed

    @property
    def pending(self) -> int:
        """Get the number of messages waiting in the queue."""
        return len(self._queue)

    def push(self, message: EmergentMessage) -> None:
        """
        Push a message to the stream.

        This is called internally by the client when messages arrive.

        Args:
            message: The message to push
        """
        if self._closed:
            return

        if self._waiting is not None and not self._waiting.done():
            self._waiting.set_result(message)
            self._waiting = None
        else:
            self._queue.append(message)

    async def next(self) -> EmergentMessage | None:
        """
        Get the next message from the stream.

        Blocks until a message is available or the stream is closed.

        Returns:
            The next message, or None if the stream is closed.
        """
        if self._closed and not self._queue:
            return None

        if self._queue:
            return self._queue.popleft()

        loop = asyncio.get_event_loop()
        self._waiting = loop.create_future()
        return await self._waiting

    def try_next(self) -> EmergentMessage | None:
        """
        Try to get the next message without blocking.

        Returns:
            The next message, or None if no message is available.
        """
        if self._closed and not self._queue:
            return None

        if self._queue:
            return self._queue.popleft()

        return None

    def close(self) -> None:
        """
        Close the stream.

        Any pending `next()` calls will resolve with None.
        Further messages pushed to the stream will be discarded.
        """
        if self._closed:
            return

        self._closed = True

        if self._waiting is not None and not self._waiting.done():
            self._waiting.set_result(None)
            self._waiting = None

        if self._on_close is not None:
            self._on_close()

    def close_with_error(self) -> None:
        """Mark the stream as closed due to an error."""
        self.close()

    def __aiter__(self) -> MessageStream:
        """Return self as the async iterator."""
        return self

    async def __anext__(self) -> EmergentMessage:
        """Get the next message or raise StopAsyncIteration."""
        if self._closed and not self._queue:
            raise StopAsyncIteration

        message = await self.next()
        if message is None:
            raise StopAsyncIteration

        return message

    async def __aenter__(self) -> MessageStream:
        """Enter the async context manager."""
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: object,
    ) -> None:
        """Exit the async context manager and close the stream."""
        self.close()
