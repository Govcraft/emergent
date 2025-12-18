"""Tests for the protocol module."""

import pytest

from emergent._protocol import (
    HEADER_SIZE,
    MAX_FRAME_SIZE,
    Format,
    MessageType,
    ProtocolVersion,
    encode_frame,
    generate_correlation_id,
    generate_message_id,
    try_decode_frame,
)
from emergent.errors import ProtocolError


class TestEncodeFrame:
    """Tests for encode_frame function."""

    def test_encode_json_frame(self) -> None:
        """Test encoding a JSON frame."""
        payload = {"key": "value", "number": 42}
        frame = encode_frame(MessageType.REQUEST, payload, Format.JSON)

        assert len(frame) > HEADER_SIZE
        # Check header
        assert frame[4] == ProtocolVersion.V2
        assert frame[5] == MessageType.REQUEST
        assert frame[6] == Format.JSON

    def test_encode_msgpack_frame(self) -> None:
        """Test encoding a MessagePack frame."""
        payload = {"key": "value", "number": 42}
        frame = encode_frame(MessageType.REQUEST, payload, Format.MSGPACK)

        assert len(frame) > HEADER_SIZE
        assert frame[4] == ProtocolVersion.V2
        assert frame[5] == MessageType.REQUEST
        assert frame[6] == Format.MSGPACK

    def test_encode_empty_payload(self) -> None:
        """Test encoding an empty payload."""
        frame = encode_frame(MessageType.REQUEST, {}, Format.JSON)
        assert len(frame) == HEADER_SIZE + 2  # "{}"

    def test_encode_complex_payload(self) -> None:
        """Test encoding a complex nested payload."""
        payload = {
            "nested": {"a": 1, "b": [1, 2, 3]},
            "list": [{"x": 1}, {"y": 2}],
        }
        frame = encode_frame(MessageType.REQUEST, payload)
        decoded = try_decode_frame(frame)

        assert decoded is not None
        assert decoded.payload == payload


class TestDecodeFrame:
    """Tests for try_decode_frame function."""

    def test_decode_json_frame(self) -> None:
        """Test decoding a JSON frame."""
        payload = {"message": "hello", "count": 123}
        frame = encode_frame(MessageType.RESPONSE, payload, Format.JSON)

        result = try_decode_frame(frame)

        assert result is not None
        assert result.msg_type == MessageType.RESPONSE
        assert result.format == Format.JSON
        assert result.payload == payload
        assert result.bytes_consumed == len(frame)

    def test_decode_msgpack_frame(self) -> None:
        """Test decoding a MessagePack frame."""
        payload = {"message": "hello", "count": 123}
        frame = encode_frame(MessageType.PUSH, payload, Format.MSGPACK)

        result = try_decode_frame(frame)

        assert result is not None
        assert result.msg_type == MessageType.PUSH
        assert result.format == Format.MSGPACK
        assert result.payload == payload

    def test_decode_incomplete_header(self) -> None:
        """Test that incomplete header returns None."""
        result = try_decode_frame(b"\x00\x00\x00")
        assert result is None

    def test_decode_incomplete_payload(self) -> None:
        """Test that incomplete payload returns None."""
        payload = {"key": "value"}
        frame = encode_frame(MessageType.REQUEST, payload)
        # Truncate the frame
        truncated = frame[: len(frame) - 5]

        result = try_decode_frame(truncated)
        assert result is None

    def test_decode_invalid_version(self) -> None:
        """Test that invalid version raises ProtocolError."""
        frame = bytearray(encode_frame(MessageType.REQUEST, {}))
        frame[4] = 0xFF  # Invalid version

        with pytest.raises(ProtocolError, match="Unsupported protocol version"):
            try_decode_frame(bytes(frame))

    def test_decode_unknown_format(self) -> None:
        """Test that unknown format raises ProtocolError."""
        frame = bytearray(encode_frame(MessageType.REQUEST, {}))
        frame[6] = 0xFF  # Invalid format

        with pytest.raises(ProtocolError, match="Unknown format"):
            try_decode_frame(bytes(frame))


class TestRoundTrip:
    """Tests for encode/decode round-trip."""

    def test_roundtrip_all_message_types(self) -> None:
        """Test round-trip for all message types."""
        payload = {"test": True}

        for msg_type in MessageType:
            frame = encode_frame(msg_type, payload)
            result = try_decode_frame(frame)

            assert result is not None
            assert result.msg_type == msg_type
            assert result.payload == payload

    def test_roundtrip_both_formats(self) -> None:
        """Test round-trip for both formats."""
        payload = {"test": True, "list": [1, 2, 3]}

        for fmt in Format:
            frame = encode_frame(MessageType.REQUEST, payload, fmt)
            result = try_decode_frame(frame)

            assert result is not None
            assert result.format == fmt
            assert result.payload == payload


class TestGenerateIds:
    """Tests for ID generation functions."""

    def test_generate_correlation_id_format(self) -> None:
        """Test correlation ID format."""
        corr_id = generate_correlation_id()
        assert corr_id.startswith("req_")
        assert len(corr_id) > 16  # Reasonable length

    def test_generate_correlation_id_custom_prefix(self) -> None:
        """Test correlation ID with custom prefix."""
        corr_id = generate_correlation_id("sub")
        assert corr_id.startswith("sub_")

    def test_generate_correlation_id_uniqueness(self) -> None:
        """Test that correlation IDs are unique."""
        ids = {generate_correlation_id() for _ in range(1000)}
        assert len(ids) == 1000

    def test_generate_message_id_format(self) -> None:
        """Test message ID format."""
        msg_id = generate_message_id()
        assert msg_id.startswith("msg_")
        assert len(msg_id) > 20  # Reasonable length

    def test_generate_message_id_custom_prefix(self) -> None:
        """Test message ID with custom prefix."""
        msg_id = generate_message_id("evt")
        assert msg_id.startswith("evt_")

    def test_generate_message_id_uniqueness(self) -> None:
        """Test that message IDs are unique."""
        ids = {generate_message_id() for _ in range(1000)}
        assert len(ids) == 1000
