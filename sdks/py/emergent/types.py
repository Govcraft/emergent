"""
Core types for the Emergent client SDK.

This module defines the public and internal types used throughout the SDK.
"""

from __future__ import annotations

from typing import Any, Literal, cast

from pydantic import BaseModel, ConfigDict, Field

# Type alias for primitive kinds
PrimitiveKind = Literal["Source", "Handler", "Sink"]
SystemPrimitiveKind = Literal["source", "handler", "sink"]


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
# System Event Types
# ============================================================================


class SystemEventPayload(BaseModel):
    """
    Payload for system lifecycle events.

    This represents the payload for `system.started.*`, `system.stopped.*`,
    and `system.error.*` events. The presence of specific fields varies:

    - `system.started.*`: Always has `pid`, never has `error`
    - `system.stopped.*`: May have `pid`, never has `error`
    - `system.error.*`: May have `pid`, always has `error`

    Attributes:
        name: Name of the primitive
        kind: Kind of the primitive ("source", "handler", or "sink")
        pid: Process ID if available
        publishes: Message types this primitive publishes
        subscribes: Message types this primitive subscribes to
        error: Error message if this is an error event

    Example:
        >>> if msg.message_type.startswith("system.started."):
        ...     payload = msg.payload_as(SystemEventPayload)
        ...     print(f"{payload.name} ({payload.kind}) started")
    """

    model_config = ConfigDict(frozen=True, extra="ignore")

    name: str
    kind: SystemPrimitiveKind
    pid: int | None = None
    publishes: list[str] = Field(default_factory=list)
    subscribes: list[str] = Field(default_factory=list)
    error: str | None = None

    def is_source(self) -> bool:
        """Return True if the primitive is a source."""
        return self.kind == "source"

    def is_handler(self) -> bool:
        """Return True if the primitive is a handler."""
        return self.kind == "handler"

    def is_sink(self) -> bool:
        """Return True if the primitive is a sink."""
        return self.kind == "sink"

    def is_error(self) -> bool:
        """Return True if this is an error event."""
        return self.error is not None


class SystemShutdownPayload(BaseModel):
    """
    Payload for `system.shutdown` events.

    This is sent by the engine when it is shutting down, signaling
    all primitives to gracefully stop. The shutdown is phased:
    1. Sources receive shutdown first
    2. Handlers receive shutdown after sources exit
    3. Sinks receive shutdown after handlers exit

    Attributes:
        kind: The type of primitive this shutdown is targeting

    Example:
        >>> if msg.message_type == "system.shutdown":
        ...     payload = msg.payload_as(SystemShutdownPayload)
        ...     if payload.is_handler_shutdown():
        ...         print("Time to gracefully exit!")
    """

    model_config = ConfigDict(frozen=True, extra="ignore")

    kind: SystemPrimitiveKind

    def is_source_shutdown(self) -> bool:
        """Return True if this shutdown is targeting sources."""
        return self.kind == "source"

    def is_handler_shutdown(self) -> bool:
        """Return True if this shutdown is targeting handlers."""
        return self.kind == "handler"

    def is_sink_shutdown(self) -> bool:
        """Return True if this shutdown is targeting sinks."""
        return self.kind == "sink"


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
