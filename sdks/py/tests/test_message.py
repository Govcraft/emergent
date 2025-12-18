"""Tests for the message module."""

import pytest

from emergent.errors import ValidationError
from emergent.message import MessageBuilder, create_message
from emergent.types import EmergentMessage


class TestMessageBuilder:
    """Tests for MessageBuilder."""

    def test_create_simple_message(self) -> None:
        """Test creating a simple message."""
        msg = create_message("timer.tick").build()

        assert isinstance(msg, EmergentMessage)
        assert msg.message_type == "timer.tick"
        assert msg.id.startswith("msg_")
        assert msg.timestamp_ms > 0
        assert msg.source == ""
        assert msg.payload is None

    def test_create_message_with_payload(self) -> None:
        """Test creating a message with payload."""
        msg = create_message("timer.tick").payload({"count": 42}).build()

        assert msg.payload == {"count": 42}

    def test_create_message_with_metadata(self) -> None:
        """Test creating a message with metadata."""
        msg = (
            create_message("timer.tick")
            .metadata({"trace_id": "abc", "location": "room-a"})
            .build()
        )

        assert msg.metadata == {"trace_id": "abc", "location": "room-a"}

    def test_create_message_with_causation(self) -> None:
        """Test creating a message with causation ID."""
        msg = create_message("order.confirmed").caused_by("msg_original_123").build()

        assert msg.causation_id == "msg_original_123"

    def test_create_message_with_correlation(self) -> None:
        """Test creating a message with correlation ID."""
        msg = create_message("order.request").correlated_with("corr_xyz").build()

        assert msg.correlation_id == "corr_xyz"

    def test_create_message_with_source(self) -> None:
        """Test creating a message with explicit source."""
        msg = create_message("sensor.reading").source("sensor_service").build()

        assert msg.source == "sensor_service"

    def test_create_message_with_custom_id(self) -> None:
        """Test creating a message with custom ID."""
        msg = create_message("timer.tick").with_id("custom_id_123").build()

        assert msg.id == "custom_id_123"

    def test_create_message_with_custom_timestamp(self) -> None:
        """Test creating a message with custom timestamp."""
        msg = create_message("timer.tick").with_timestamp(1234567890123).build()

        assert msg.timestamp_ms == 1234567890123

    def test_fluent_chaining(self) -> None:
        """Test full fluent chaining."""
        msg = (
            create_message("order.confirmed")
            .payload({"order_id": "123", "status": "ok"})
            .metadata({"trace_id": "abc"})
            .caused_by("msg_original")
            .correlated_with("corr_123")
            .source("order_processor")
            .build()
        )

        assert msg.message_type == "order.confirmed"
        assert msg.payload == {"order_id": "123", "status": "ok"}
        assert msg.metadata == {"trace_id": "abc"}
        assert msg.causation_id == "msg_original"
        assert msg.correlation_id == "corr_123"
        assert msg.source == "order_processor"

    def test_empty_message_type_raises(self) -> None:
        """Test that empty message type raises ValidationError."""
        with pytest.raises(ValidationError, match="Message type cannot be empty"):
            create_message("")

    def test_whitespace_message_type_raises(self) -> None:
        """Test that whitespace-only message type raises ValidationError."""
        with pytest.raises(ValidationError, match="Message type cannot be empty"):
            create_message("   ")

    def test_builder_returns_self(self) -> None:
        """Test that builder methods return self for chaining."""
        builder = create_message("test")

        assert builder.payload({"a": 1}) is builder
        assert builder.metadata({"b": 2}) is builder
        assert builder.caused_by("msg_123") is builder
        assert builder.correlated_with("corr_123") is builder
        assert builder.source("src") is builder
        assert builder.with_id("id") is builder
        assert builder.with_timestamp(123) is builder

    def test_build_creates_immutable_message(self) -> None:
        """Test that build creates an immutable message."""
        msg = create_message("timer.tick").payload({"count": 1}).build()

        # Message should be frozen
        with pytest.raises(Exception):
            msg.payload = {"count": 2}  # type: ignore[misc]

    def test_multiple_builds_create_different_messages(self) -> None:
        """Test that calling build multiple times works."""
        builder = create_message("timer.tick")

        msg1 = builder.payload({"count": 1}).build()
        msg2 = builder.payload({"count": 2}).build()

        # Both should have same ID since it's the same builder
        assert msg1.id == msg2.id
        # Messages are independent once built (Pydantic frozen model)
        assert msg1.payload == {"count": 1}
        assert msg2.payload == {"count": 2}


class TestCreateMessage:
    """Tests for create_message function."""

    def test_creates_message_builder(self) -> None:
        """Test that create_message returns a MessageBuilder."""
        builder = create_message("timer.tick")

        assert isinstance(builder, MessageBuilder)

    def test_various_message_types(self) -> None:
        """Test creating messages with various type formats."""
        types = [
            "timer.tick",
            "order.created",
            "user.signed_up",
            "system.shutdown",
            "a.b.c.d.e",
            "single",
        ]

        for msg_type in types:
            msg = create_message(msg_type).build()
            assert msg.message_type == msg_type
