"""
IPC protocol implementation matching acton-reactive.

This module handles frame encoding/decoding for the binary IPC protocol.
"""

from __future__ import annotations

import json
import struct
from dataclasses import dataclass
from enum import IntEnum
from typing import Any

import msgpack
from typeid import TypeID

from .errors import ProtocolError


# Protocol Constants (matching acton-reactive IPC)
class ProtocolVersion(IntEnum):
    """Protocol version constants."""

    V2 = 0x02


class MessageType(IntEnum):
    """
    IPC message type constants.

    Mirrors the ``MSG_TYPE_*`` table in acton-reactive's
    ``common/ipc/protocol.rs`` one for one. The engine answers a failed request
    with an ``ERROR`` frame in place of a ``RESPONSE`` frame, and the body of
    both is the same response object.
    """

    REQUEST = 0x01
    RESPONSE = 0x02
    ERROR = 0x03
    HEARTBEAT = 0x04
    PUSH = 0x05
    SUBSCRIBE = 0x06
    UNSUBSCRIBE = 0x07
    DISCOVER = 0x08
    STREAM = 0x09
    SUBSCRIBE_PATTERNS = 0x0A
    UNSUBSCRIBE_PATTERNS = 0x0B


class Format(IntEnum):
    """Serialization format constants."""

    JSON = 0x01
    MSGPACK = 0x02


#: Wire format every client uses unless told otherwise. MessagePack matches the
#: Rust, TypeScript and Go SDKs and is what the engine itself sends.
DEFAULT_FORMAT = Format.MSGPACK


# Frame header size: length(4) + version(1) + msgType(1) + format(1)
HEADER_SIZE = 7

# Maximum frame size (16 MiB)
MAX_FRAME_SIZE = 16 * 1024 * 1024


@dataclass(frozen=True)
class DecodedFrame:
    """
    Result of decoding a frame.

    ``msg_type`` is ``None`` when the type byte is not in ``MessageType``. The
    frame is still delimited by its length prefix, so a reader can skip it and
    stay in step with the stream. ``raw_msg_type`` always holds the byte read.
    """

    msg_type: MessageType | None
    format: Format
    payload: Any
    bytes_consumed: int
    raw_msg_type: int


@dataclass(frozen=True)
class BadFrameBody:
    """
    A whole frame whose body does not decode.

    Its length is known, so a reader drops ``bytes_consumed`` bytes and carries
    on with the next frame.
    """

    raw_msg_type: int
    bytes_consumed: int
    reason: str


@dataclass(frozen=True)
class BadFraming:
    """A header that cannot be trusted, so the next frame cannot be found."""

    reason: str


def parse_message_type(raw: int) -> MessageType | None:
    """
    Map a wire byte to its ``MessageType``.

    Args:
        raw: The message type byte from a frame header

    Returns:
        The matching ``MessageType``, or None for a byte this SDK does not know
    """
    try:
        return MessageType(raw)
    except ValueError:
        return None


def decode_payload(payload_bytes: bytes | bytearray, format_raw: int) -> Any:
    """
    Decode a frame body.

    An empty body decodes to None. A heartbeat frame is a bare header, and
    neither JSON nor MessagePack can parse zero bytes.

    Args:
        payload_bytes: The frame body
        format_raw: The serialization format byte from the frame header

    Returns:
        The decoded payload, or None for an empty body

    Raises:
        ProtocolError: If the format byte is unknown, or the body is not valid
            in its format, such as one cut short
    """
    if format_raw not in (Format.JSON, Format.MSGPACK):
        raise ProtocolError(f"Unknown format: {format_raw}")

    if len(payload_bytes) == 0:
        return None

    # The decoders raise their own errors: ValueError and its subclasses from
    # JSON and UTF-8, and msgpack's own family. Callers handle one class.
    try:
        if format_raw == Format.JSON:
            return json.loads(bytes(payload_bytes).decode("utf-8"))
        return msgpack.unpackb(payload_bytes, raw=False)
    except (ValueError, msgpack.UnpackException, RecursionError) as e:
        name = "JSON" if format_raw == Format.JSON else "MessagePack"
        raise ProtocolError(f"Malformed {name} frame body: {e}") from e


def encode_frame(
    msg_type: MessageType,
    payload: Any,
    format_: Format = DEFAULT_FORMAT,
) -> bytes:
    """
    Encode a frame for transmission.

    Frame structure:
    - [0-3]: Payload length (big-endian u32)
    - [4]: Protocol version
    - [5]: Message type
    - [6]: Serialization format
    - [7+]: Payload bytes

    Args:
        msg_type: The message type constant
        payload: The payload to serialize
        format_: The serialization format (JSON or MSGPACK)

    Returns:
        The encoded frame as bytes

    Raises:
        ProtocolError: If payload is too large or format is unsupported
    """
    if format_ == Format.JSON:
        payload_bytes = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    elif format_ == Format.MSGPACK:
        payload_bytes = msgpack.packb(payload, use_bin_type=True)
    else:
        raise ProtocolError(f"Unsupported format: {format_}")

    payload_len = len(payload_bytes)

    if payload_len > MAX_FRAME_SIZE:
        raise ProtocolError(f"Payload too large: {payload_len} bytes (max: {MAX_FRAME_SIZE})")

    # Pack header: big-endian u32 length + 3 bytes (version, type, format)
    header = struct.pack(
        ">IBBB",
        payload_len,
        ProtocolVersion.V2,
        msg_type,
        format_,
    )

    return header + payload_bytes


def next_frame(buffer: bytes | bytearray) -> DecodedFrame | BadFrameBody | BadFraming | None:
    """
    Read the frame at the front of a buffer without raising.

    A body that does not decode is told apart from a header that cannot be
    trusted, because only the first leaves the reader able to find the next
    frame.

    Args:
        buffer: The buffer to read from

    Returns:
        The decoded frame, ``BadFrameBody`` or ``BadFraming`` for a frame that
        is malformed, or None if the buffer does not hold a whole frame yet
    """
    if len(buffer) < HEADER_SIZE:
        return None  # Not enough data for header

    # Unpack header
    payload_len, version, msg_type_raw, format_raw = struct.unpack(">IBBB", buffer[:HEADER_SIZE])

    if payload_len > MAX_FRAME_SIZE:
        return BadFraming(f"Frame too large: {payload_len} bytes")

    if version != ProtocolVersion.V2:
        return BadFraming(
            f"Unsupported protocol version: {version} (expected {ProtocolVersion.V2})"
        )

    total_len = HEADER_SIZE + payload_len

    if len(buffer) < total_len:
        return None  # Not enough data for full frame

    try:
        payload = decode_payload(buffer[HEADER_SIZE:total_len], format_raw)
    except ProtocolError as e:
        return BadFrameBody(
            raw_msg_type=msg_type_raw,
            bytes_consumed=total_len,
            reason=str(e),
        )

    return DecodedFrame(
        msg_type=parse_message_type(msg_type_raw),
        format=Format(format_raw),
        payload=payload,
        bytes_consumed=total_len,
        raw_msg_type=msg_type_raw,
    )


def try_decode_frame(buffer: bytes | bytearray) -> DecodedFrame | None:
    """
    Try to decode a frame from a buffer.

    Returns None if the buffer doesn't contain a complete frame.

    Args:
        buffer: The buffer to decode from

    Returns:
        DecodedFrame if successful, None if not enough data

    Raises:
        ProtocolError: If the frame is malformed
    """
    step = next_frame(buffer)
    if isinstance(step, (BadFrameBody, BadFraming)):
        raise ProtocolError(step.reason)
    return step


def generate_correlation_id(prefix: str = "req") -> str:
    """
    Generate a unique correlation ID in TypeID format.

    Format: `{prefix}_{base32_crockford_uuidv7}`

    Args:
        prefix: Prefix for the ID (e.g., "cor", "sub", "pub")

    Returns:
        A unique correlation ID string
    """
    return str(TypeID(prefix))


def generate_message_id(prefix: str = "msg") -> str:
    """
    Generate a unique message ID in TypeID format.

    Format: `{prefix}_{base32_crockford_uuidv7}`

    Args:
        prefix: Prefix for the ID (default: "msg")

    Returns:
        A unique message ID string
    """
    return str(TypeID(prefix))
