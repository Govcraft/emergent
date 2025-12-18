"""
IPC protocol implementation matching acton-reactive.

This module handles frame encoding/decoding for the binary IPC protocol.
"""

from __future__ import annotations

import json
import secrets
import struct
import time
from dataclasses import dataclass
from enum import IntEnum
from typing import Any

import msgpack

from .errors import ProtocolError


# Protocol Constants (matching acton-reactive IPC)
class ProtocolVersion(IntEnum):
    """Protocol version constants."""

    V2 = 0x02


class MessageType(IntEnum):
    """IPC message type constants."""

    REQUEST = 0x01
    RESPONSE = 0x02
    DISCOVER = 0x03
    PUSH = 0x05
    SUBSCRIBE = 0x06
    UNSUBSCRIBE = 0x07


class Format(IntEnum):
    """Serialization format constants."""

    JSON = 0x01
    MSGPACK = 0x02


# Frame header size: length(4) + version(1) + msgType(1) + format(1)
HEADER_SIZE = 7

# Maximum frame size (16 MiB)
MAX_FRAME_SIZE = 16 * 1024 * 1024


@dataclass(frozen=True)
class DecodedFrame:
    """Result of decoding a frame."""

    msg_type: MessageType
    format: Format
    payload: Any
    bytes_consumed: int


def encode_frame(
    msg_type: MessageType,
    payload: Any,
    format_: Format = Format.JSON,
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
        raise ProtocolError(
            f"Payload too large: {payload_len} bytes (max: {MAX_FRAME_SIZE})"
        )

    # Pack header: big-endian u32 length + 3 bytes (version, type, format)
    header = struct.pack(
        ">IBBB",
        payload_len,
        ProtocolVersion.V2,
        msg_type,
        format_,
    )

    return header + payload_bytes


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
    if len(buffer) < HEADER_SIZE:
        return None  # Not enough data for header

    # Unpack header
    payload_len, version, msg_type_raw, format_raw = struct.unpack(
        ">IBBB", buffer[:HEADER_SIZE]
    )

    if payload_len > MAX_FRAME_SIZE:
        raise ProtocolError(f"Frame too large: {payload_len} bytes")

    total_len = HEADER_SIZE + payload_len

    if len(buffer) < total_len:
        return None  # Not enough data for full frame

    if version != ProtocolVersion.V2:
        raise ProtocolError(
            f"Unsupported protocol version: {version} (expected {ProtocolVersion.V2})"
        )

    payload_bytes = buffer[HEADER_SIZE:total_len]

    if format_raw == Format.JSON:
        payload = json.loads(payload_bytes.decode("utf-8"))
    elif format_raw == Format.MSGPACK:
        payload = msgpack.unpackb(payload_bytes, raw=False)
    else:
        raise ProtocolError(f"Unknown format: {format_raw}")

    return DecodedFrame(
        msg_type=MessageType(msg_type_raw),
        format=Format(format_raw),
        payload=payload,
        bytes_consumed=total_len,
    )


def generate_correlation_id(prefix: str = "req") -> str:
    """
    Generate a unique correlation ID.

    Format: `{prefix}_{timestamp_hex}{random_hex}`

    Args:
        prefix: Prefix for the ID (e.g., "req", "sub", "pub")

    Returns:
        A unique correlation ID string
    """
    timestamp_hex = format(int(time.time() * 1000), "012x")
    random_hex = secrets.token_hex(4)
    return f"{prefix}_{timestamp_hex}{random_hex}"


def generate_message_id(prefix: str = "msg") -> str:
    """
    Generate a unique message ID in MTI format.

    Format: `{prefix}_{timestamp_hex}{random_hex}`
    This produces time-ordered, unique IDs similar to UUIDv7.

    Args:
        prefix: Prefix for the ID (default: "msg")

    Returns:
        A unique message ID string
    """
    timestamp_hex = format(int(time.time() * 1000), "012x")
    random_hex = secrets.token_hex(8)
    return f"{prefix}_{timestamp_hex}{random_hex}"
