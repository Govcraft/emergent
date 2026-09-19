"""Tests for the protocol module."""

import struct

import pytest

from emergent._protocol import (
    HEADER_SIZE,
    Format,
    MessageType,
    ProtocolVersion,
    decode_payload,
    encode_frame,
    generate_correlation_id,
    generate_message_id,
    parse_message_type,
    try_decode_frame,
)
from emergent.errors import ProtocolError

# The MSG_TYPE_* table from acton-reactive 9.3.0, common/ipc/protocol.rs.
ACTON_MESSAGE_TYPES = {
    "REQUEST": 0x01,
    "RESPONSE": 0x02,
    "ERROR": 0x03,
    "HEARTBEAT": 0x04,
    "PUSH": 0x05,
    "SUBSCRIBE": 0x06,
    "UNSUBSCRIBE": 0x07,
    "DISCOVER": 0x08,
    "STREAM": 0x09,
    "SUBSCRIBE_PATTERNS": 0x0A,
    "UNSUBSCRIBE_PATTERNS": 0x0B,
}


def bare_frame(msg_type: int, body: bytes = b"", format_: int = Format.JSON) -> bytes:
    """Build a frame by hand, so a test can use bytes the encoder would not."""
    return struct.pack(">IBBB", len(body), ProtocolVersion.V2, msg_type, format_) + body


class TestOpcodeTable:
    """The enum must match acton-reactive's table value for value."""

    def test_table_matches_acton_exactly(self) -> None:
        assert {member.name: member.value for member in MessageType} == ACTON_MESSAGE_TYPES

    def test_no_two_names_share_a_value(self) -> None:
        # IntEnum turns a repeated value into an alias, which iteration hides.
        assert len(MessageType.__members__) == len(ACTON_MESSAGE_TYPES)

    def test_discover_is_not_the_error_opcode(self) -> None:
        assert MessageType.DISCOVER == 0x08
        assert MessageType.ERROR == 0x03

    @pytest.mark.parametrize(("name", "value"), sorted(ACTON_MESSAGE_TYPES.items()))
    def test_each_type_is_the_byte_on_the_wire(self, name: str, value: int) -> None:
        frame = encode_frame(MessageType[name], {})
        assert frame[5] == value

    @pytest.mark.parametrize(("name", "value"), sorted(ACTON_MESSAGE_TYPES.items()))
    def test_each_byte_parses_back(self, name: str, value: int) -> None:
        assert parse_message_type(value) is MessageType[name]

    @pytest.mark.parametrize("raw", [0x00, 0x0C, 0x7F, 0xFF])
    def test_unknown_byte_parses_to_none(self, raw: int) -> None:
        assert parse_message_type(raw) is None


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


class TestDecodeFrameTypes:
    """Frames the engine sends that the decoder once mishandled."""

    def test_error_frame_decodes_as_error(self) -> None:
        body = b'{"correlation_id":"req_x","success":false,"error":"Actor not found: nope"}'
        decoded = try_decode_frame(bare_frame(0x03, body))

        assert decoded is not None
        assert decoded.msg_type is MessageType.ERROR
        assert decoded.payload["error"] == "Actor not found: nope"

    @pytest.mark.parametrize("format_", [Format.JSON, Format.MSGPACK])
    def test_heartbeat_with_no_body_decodes(self, format_: Format) -> None:
        # The engine writes a heartbeat as a bare header marked JSON.
        decoded = try_decode_frame(bare_frame(0x04, b"", format_))

        assert decoded is not None
        assert decoded.msg_type is MessageType.HEARTBEAT
        assert decoded.payload is None
        assert decoded.bytes_consumed == HEADER_SIZE

    def test_unknown_type_is_delimited_not_fatal(self) -> None:
        unknown = bare_frame(0x0C, b'{"later":"protocol"}')
        decoded = try_decode_frame(unknown)

        assert decoded is not None
        assert decoded.msg_type is None
        assert decoded.raw_msg_type == 0x0C
        assert decoded.bytes_consumed == len(unknown)

    def test_frame_after_an_unknown_type_still_decodes(self) -> None:
        unknown = bare_frame(0x0C, b"{}")
        buffer = unknown + encode_frame(MessageType.PUSH, {"n": 1})

        first = try_decode_frame(buffer)
        assert first is not None
        second = try_decode_frame(buffer[first.bytes_consumed :])

        assert second is not None
        assert second.msg_type is MessageType.PUSH
        assert second.payload == {"n": 1}

    def test_empty_body_with_unknown_format_is_still_rejected(self) -> None:
        with pytest.raises(ProtocolError, match="Unknown format"):
            decode_payload(b"", 0xFF)


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
