"""Tests for the types module."""

import pytest
from pydantic import BaseModel

from emergent.types import (
    DiscoveryInfo,
    EmergentMessage,
    IpcEnvelope,
    IpcPushNotification,
    IpcResponse,
    PrimitiveInfo,
    WireMessage,
)


class TestEmergentMessage:
    """Tests for EmergentMessage."""

    def test_create_minimal_message(self) -> None:
        """Test creating a message with minimal fields."""
        msg = EmergentMessage(
            id="msg_123",
            message_type="test.event",
            source="test_source",
            timestamp_ms=1234567890123,
        )

        assert msg.id == "msg_123"
        assert msg.message_type == "test.event"
        assert msg.source == "test_source"
        assert msg.timestamp_ms == 1234567890123
        assert msg.payload is None
        assert msg.correlation_id is None
        assert msg.causation_id is None
        assert msg.metadata is None

    def test_create_full_message(self) -> None:
        """Test creating a message with all fields."""
        msg = EmergentMessage(
            id="msg_123",
            message_type="test.event",
            source="test_source",
            timestamp_ms=1234567890123,
            payload={"key": "value"},
            correlation_id="corr_456",
            causation_id="msg_789",
            metadata={"trace_id": "abc"},
        )

        assert msg.payload == {"key": "value"}
        assert msg.correlation_id == "corr_456"
        assert msg.causation_id == "msg_789"
        assert msg.metadata == {"trace_id": "abc"}

    def test_message_is_frozen(self) -> None:
        """Test that messages are immutable."""
        msg = EmergentMessage(
            id="msg_123",
            message_type="test.event",
            source="test_source",
            timestamp_ms=1234567890123,
        )

        with pytest.raises(Exception):  # ValidationError from Pydantic
            msg.id = "different_id"  # type: ignore[misc]

    def test_payload_as_dict(self) -> None:
        """Test payload_as with dict type."""
        msg = EmergentMessage(
            id="msg_123",
            message_type="test.event",
            source="test_source",
            timestamp_ms=1234567890123,
            payload={"count": 42, "name": "test"},
        )

        result = msg.payload_as(dict)
        assert result == {"count": 42, "name": "test"}

    def test_payload_as_model(self) -> None:
        """Test payload_as with Pydantic model."""

        class SensorReading(BaseModel):
            value: float
            unit: str

        msg = EmergentMessage(
            id="msg_123",
            message_type="sensor.reading",
            source="sensor_1",
            timestamp_ms=1234567890123,
            payload={"value": 42.5, "unit": "celsius"},
        )

        reading = msg.payload_as(SensorReading)
        assert isinstance(reading, SensorReading)
        assert reading.value == 42.5
        assert reading.unit == "celsius"

    def test_to_wire(self) -> None:
        """Test converting to wire format."""
        msg = EmergentMessage(
            id="msg_123",
            message_type="test.event",
            source="test_source",
            timestamp_ms=1234567890123,
            payload={"key": "value"},
            correlation_id="corr_456",
        )

        wire = msg.to_wire()

        assert isinstance(wire, WireMessage)
        assert wire.id == "msg_123"
        assert wire.message_type == "test.event"
        assert wire.source == "test_source"
        assert wire.timestamp_ms == 1234567890123
        assert wire.payload == {"key": "value"}
        assert wire.correlation_id == "corr_456"

    def test_from_wire(self) -> None:
        """Test creating from wire format."""
        wire = WireMessage(
            id="msg_123",
            message_type="test.event",
            source="test_source",
            timestamp_ms=1234567890123,
            payload={"key": "value"},
        )

        msg = EmergentMessage.from_wire(wire)

        assert msg.id == "msg_123"
        assert msg.message_type == "test.event"
        assert msg.source == "test_source"
        assert msg.payload == {"key": "value"}

    def test_roundtrip_wire_conversion(self) -> None:
        """Test round-trip wire conversion."""
        original = EmergentMessage(
            id="msg_123",
            message_type="test.event",
            source="test_source",
            timestamp_ms=1234567890123,
            payload={"key": "value"},
            correlation_id="corr_456",
            causation_id="msg_789",
            metadata={"trace": "abc"},
        )

        wire = original.to_wire()
        restored = EmergentMessage.from_wire(wire)

        assert restored.id == original.id
        assert restored.message_type == original.message_type
        assert restored.source == original.source
        assert restored.timestamp_ms == original.timestamp_ms
        assert restored.payload == original.payload
        assert restored.correlation_id == original.correlation_id
        assert restored.causation_id == original.causation_id
        assert restored.metadata == original.metadata


class TestPrimitiveInfo:
    """Tests for PrimitiveInfo."""

    def test_create_primitive_info(self) -> None:
        """Test creating primitive info."""
        info = PrimitiveInfo(name="my_handler", kind="Handler")

        assert info.name == "my_handler"
        assert info.kind == "Handler"

    def test_primitive_info_is_frozen(self) -> None:
        """Test that primitive info is immutable."""
        info = PrimitiveInfo(name="my_handler", kind="Handler")

        with pytest.raises(Exception):
            info.name = "different"  # type: ignore[misc]


class TestDiscoveryInfo:
    """Tests for DiscoveryInfo."""

    def test_create_discovery_info(self) -> None:
        """Test creating discovery info."""
        info = DiscoveryInfo(
            message_types=("timer.tick", "order.created"),
            primitives=(
                PrimitiveInfo(name="source_1", kind="Source"),
                PrimitiveInfo(name="handler_1", kind="Handler"),
            ),
        )

        assert info.message_types == ("timer.tick", "order.created")
        assert len(info.primitives) == 2
        assert info.primitives[0].name == "source_1"
        assert info.primitives[1].kind == "Handler"


class TestIpcTypes:
    """Tests for IPC types."""

    def test_ipc_envelope(self) -> None:
        """Test IPC envelope creation."""
        envelope = IpcEnvelope(
            correlation_id="req_123",
            target="broker",
            message_type="Publish",
            payload={"data": "value"},
            expects_reply=True,
        )

        assert envelope.correlation_id == "req_123"
        assert envelope.target == "broker"
        assert envelope.message_type == "Publish"
        assert envelope.payload == {"data": "value"}
        assert envelope.expects_reply is True

    def test_ipc_response_success(self) -> None:
        """Test IPC response for success."""
        response = IpcResponse(
            correlation_id="req_123",
            success=True,
            payload={"result": "ok"},
        )

        assert response.success is True
        assert response.payload == {"result": "ok"}
        assert response.error is None

    def test_ipc_response_error(self) -> None:
        """Test IPC response for error."""
        response = IpcResponse(
            correlation_id="req_123",
            success=False,
            error="Something went wrong",
            error_code="INTERNAL_ERROR",
        )

        assert response.success is False
        assert response.error == "Something went wrong"
        assert response.error_code == "INTERNAL_ERROR"

    def test_ipc_push_notification(self) -> None:
        """Test IPC push notification."""
        notification = IpcPushNotification(
            notification_id="notif_123",
            message_type="timer.tick",
            payload={"count": 1},
            source_actor="timer_actor",
            timestamp_ms=1234567890123,
        )

        assert notification.notification_id == "notif_123"
        assert notification.message_type == "timer.tick"
        assert notification.payload == {"count": 1}
