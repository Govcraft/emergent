"""
Core types for the Emergent client SDK.

This module defines the public and internal types used throughout the SDK.
"""

from __future__ import annotations

from typing import Any, Literal, cast

from pydantic import BaseModel, ConfigDict

# Type alias for primitive kinds
PrimitiveKind = Literal["Source", "Handler", "Sink"]


# ============================================================================
# Public Types
# ============================================================================


class EmergentMessage(BaseModel):
    """
    Standard Emergent message envelope.

    Messages are immutable once created. Use `create_message()` to create
    new messages with the fluent builder pattern.

    Attributes:
        id: Unique message ID (MTI format: msg_<timestamp_hex><random_hex>)
        message_type: Message type for routing (e.g., "timer.tick")
        source: Source client that published this message
        correlation_id: Optional correlation ID for request-response patterns
        causation_id: Optional causation ID (ID of message that triggered this one)
        timestamp_ms: Timestamp when message was created (Unix ms)
        payload: User-defined payload
        metadata: Optional metadata for tracing/debugging

    Example:
        >>> msg = EmergentMessage(
        ...     id="msg_abc123",
        ...     message_type="timer.tick",
        ...     source="timer_service",
        ...     timestamp_ms=1234567890123,
        ...     payload={"count": 1},
        ... )
        >>> data = msg.payload_as(dict)
    """

    model_config = ConfigDict(frozen=True, extra="forbid")

    id: str
    message_type: str
    source: str
    correlation_id: str | None = None
    causation_id: str | None = None
    timestamp_ms: int
    payload: Any = None
    metadata: dict[str, Any] | None = None

    def payload_as[T](self, model: type[T]) -> T:
        """
        Get the payload as a specific type.

        If the payload is a dict and `model` is a Pydantic BaseModel,
        it will be validated and converted. Otherwise, the payload
        is cast directly.

        Args:
            model: The type to convert the payload to

        Returns:
            The payload as the specified type

        Example:
            >>> class SensorReading(BaseModel):
            ...     value: float
            ...     unit: str
            >>> reading = msg.payload_as(SensorReading)
            >>> print(reading.value, reading.unit)
        """
        if isinstance(self.payload, dict) and issubclass(model, BaseModel):
            return model.model_validate(self.payload)
        return cast("T", self.payload)

    def to_wire(self) -> WireMessage:
        """Convert to wire format for serialization."""
        return WireMessage(
            id=self.id,
            message_type=self.message_type,
            source=self.source,
            correlation_id=self.correlation_id,
            causation_id=self.causation_id,
            timestamp_ms=self.timestamp_ms,
            payload=self.payload,
            metadata=self.metadata,
        )

    @classmethod
    def from_wire(cls, wire: WireMessage) -> EmergentMessage:
        """Create from wire format."""
        return cls(
            id=wire.id,
            message_type=wire.message_type,
            source=wire.source,
            correlation_id=wire.correlation_id,
            causation_id=wire.causation_id,
            timestamp_ms=wire.timestamp_ms,
            payload=wire.payload,
            metadata=wire.metadata if isinstance(wire.metadata, dict) else None,
        )


class PrimitiveInfo(BaseModel):
    """
    Information about a registered primitive.

    Attributes:
        name: Name of the primitive
        kind: Type of primitive (Source, Handler, Sink)
    """

    model_config = ConfigDict(frozen=True)

    name: str
    kind: PrimitiveKind


class DiscoveryInfo(BaseModel):
    """
    Discovery information about the engine.

    Attributes:
        message_types: Available message types that can be subscribed to
        primitives: List of connected primitives
    """

    model_config = ConfigDict(frozen=True)

    message_types: tuple[str, ...]
    primitives: tuple[PrimitiveInfo, ...]


class ConnectOptions(BaseModel):
    """
    Options for connecting to the Emergent engine.

    Attributes:
        socket_path: Custom socket path (overrides EMERGENT_SOCKET env var)
        timeout: Connection timeout in seconds (default: 30.0)
    """

    socket_path: str | None = None
    timeout: float = 30.0


# ============================================================================
# Internal Types (wire format)
# ============================================================================


class WireMessage(BaseModel):
    """
    Wire format message (snake_case for JSON serialization).

    This matches the format expected by the Rust server.
    """

    model_config = ConfigDict(extra="forbid")

    id: str
    message_type: str
    source: str
    correlation_id: str | None = None
    causation_id: str | None = None
    timestamp_ms: int
    payload: Any = None
    metadata: Any | None = None


class IpcEnvelope(BaseModel):
    """Request envelope sent to the server."""

    correlation_id: str
    target: str
    message_type: str
    payload: Any
    expects_reply: bool = True


class IpcResponse(BaseModel):
    """Response from the server."""

    correlation_id: str
    success: bool
    payload: Any | None = None
    error: str | None = None
    error_code: str | None = None


class IpcSubscribeRequest(BaseModel):
    """Subscribe request payload."""

    correlation_id: str
    message_types: list[str]


class IpcSubscriptionResponse(BaseModel):
    """Subscription response."""

    success: bool
    subscribed_types: list[str]
    error: str | None = None


class IpcPushNotification(BaseModel):
    """Push notification from subscriptions."""

    notification_id: str
    message_type: str
    payload: Any
    source_actor: str | None = None
    timestamp_ms: int = 0


class IpcDiscoverResponse(BaseModel):
    """Discovery response payload."""

    message_types: list[str]
    primitives: list[dict[str, str]]
