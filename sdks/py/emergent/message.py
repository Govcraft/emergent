"""
Message creation and builder utilities.

This module provides the fluent builder pattern for creating messages.
"""

from __future__ import annotations

import time
from typing import Any, Self

from ._protocol import generate_message_id
from .errors import ValidationError
from .types import EmergentMessage


class MessageBuilder:
    """
    Fluent builder for creating EmergentMessage instances.

    Example:
        >>> # Simple message
        >>> msg = create_message("timer.tick").payload({"count": 1}).build()

        >>> # With causation tracking
        >>> reply = (
        ...     create_message("order.confirmed")
        ...     .caused_by(original_msg.id)
        ...     .payload({"confirmed": True})
        ...     .build()
        ... )

        >>> # With all options
        >>> msg = (
        ...     create_message("sensor.reading")
        ...     .payload({"value": 42.5, "unit": "celsius"})
        ...     .metadata({"sensor_id": "temp-01", "location": "room-a"})
        ...     .source("sensor_service")
        ...     .build()
        ... )
    """

    def __init__(self, message_type: str) -> None:
        """
        Create a new message builder.

        Args:
            message_type: The message type (e.g., "timer.tick")

        Raises:
            ValidationError: If message_type is empty
        """
        if not message_type or not message_type.strip():
            raise ValidationError("Message type cannot be empty", "message_type")

        self._id = generate_message_id()
        self._message_type = message_type
        self._source = ""
        self._correlation_id: str | None = None
        self._causation_id: str | None = None
        self._timestamp_ms = int(time.time() * 1000)
        self._payload: Any = None
        self._metadata: dict[str, Any] | None = None

    def payload(self, data: Any) -> Self:
        """
        Set the message payload.

        Args:
            data: The payload data (will be JSON serialized)

        Returns:
            Self for method chaining
        """
        self._payload = data
        return self

    def metadata(self, meta: dict[str, Any]) -> Self:
        """
        Set the message metadata.

        Args:
            meta: Metadata for tracing/debugging

        Returns:
            Self for method chaining
        """
        self._metadata = meta
        return self

    def caused_by(self, message_id: str) -> Self:
        """
        Set the causation ID (for message chain tracking).

        Use this when creating a message in response to another message.

        Args:
            message_id: The ID of the message that caused this one

        Returns:
            Self for method chaining
        """
        self._causation_id = message_id
        return self

    def correlated_with(self, correlation_id: str) -> Self:
        """
        Set the correlation ID (for request-response patterns).

        Args:
            correlation_id: The correlation ID to link related messages

        Returns:
            Self for method chaining
        """
        self._correlation_id = correlation_id
        return self

    def source(self, name: str) -> Self:
        """
        Set the source name.

        Note: This is typically set automatically by the client when publishing.

        Args:
            name: The source name

        Returns:
            Self for method chaining
        """
        self._source = name
        return self

    def with_id(self, id_: str) -> Self:
        """
        Set a custom message ID.

        Note: In most cases you should not use this. IDs are auto-generated.

        Args:
            id_: The message ID

        Returns:
            Self for method chaining
        """
        self._id = id_
        return self

    def with_timestamp(self, timestamp_ms: int) -> Self:
        """
        Set a custom timestamp.

        Note: In most cases you should not use this. Timestamps are auto-generated.

        Args:
            timestamp_ms: Unix timestamp in milliseconds

        Returns:
            Self for method chaining
        """
        self._timestamp_ms = timestamp_ms
        return self

    def build(self) -> EmergentMessage:
        """
        Build the message.

        Returns:
            A frozen, immutable EmergentMessage
        """
        return EmergentMessage(
            id=self._id,
            message_type=self._message_type,
            source=self._source,
            correlation_id=self._correlation_id,
            causation_id=self._causation_id,
            timestamp_ms=self._timestamp_ms,
            payload=self._payload,
            metadata=self._metadata,
        )


def create_message(message_type: str) -> MessageBuilder:
    """
    Create a new message builder.

    This is the primary way to create new messages for publishing.

    Args:
        message_type: The message type (e.g., "timer.tick")

    Returns:
        A MessageBuilder instance

    Example:
        >>> # Simple message
        >>> msg = create_message("timer.tick").payload({"count": 1}).build()

        >>> # Shorthand with source.publish()
        >>> await source.publish(create_message("timer.tick").payload({"count": 1}))
    """
    return MessageBuilder(message_type)
