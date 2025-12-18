/**
 * IPC protocol implementation matching acton-reactive.
 * @module
 */

import { encode, decode } from "npm:@msgpack/msgpack@3.0.0";
import { ProtocolError } from "./errors.ts";

// ============================================================================
// Protocol Constants (matching acton-reactive IPC)
// ============================================================================

/** Protocol version */
export const PROTOCOL_VERSION = 0x02;

/** Maximum frame size (16 MiB) */
export const MAX_FRAME_SIZE = 16 * 1024 * 1024;

/** Frame header size: length(4) + version(1) + msgType(1) + format(1) */
export const HEADER_SIZE = 7;

// Message types (matching acton-reactive/src/common/ipc/protocol.rs)
export const MSG_TYPE_REQUEST = 0x01;
export const MSG_TYPE_RESPONSE = 0x02;
export const MSG_TYPE_ERROR = 0x03;
export const MSG_TYPE_HEARTBEAT = 0x04;
export const MSG_TYPE_PUSH = 0x05;
export const MSG_TYPE_SUBSCRIBE = 0x06;
export const MSG_TYPE_UNSUBSCRIBE = 0x07;
export const MSG_TYPE_DISCOVER = 0x08;
export const MSG_TYPE_STREAM = 0x09;

// Serialization formats
export const FORMAT_JSON = 0x01;
export const FORMAT_MSGPACK = 0x02;

// ============================================================================
// Encoding/Decoding
// ============================================================================

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

/**
 * Encode a frame for transmission.
 *
 * Frame structure:
 * - [0-3]: Payload length (big-endian u32)
 * - [4]: Protocol version
 * - [5]: Message type
 * - [6]: Serialization format
 * - [7+]: Payload bytes
 */
export function encodeFrame(
  msgType: number,
  payload: unknown,
  format = FORMAT_MSGPACK,
): Uint8Array {
  let payloadBytes: Uint8Array;

  if (format === FORMAT_MSGPACK) {
    payloadBytes = encode(payload);
  } else if (format === FORMAT_JSON) {
    const jsonStr = JSON.stringify(payload);
    payloadBytes = textEncoder.encode(jsonStr);
  } else {
    throw new ProtocolError(`Unsupported format: ${format}`);
  }

  const payloadLen = payloadBytes.length;

  if (payloadLen > MAX_FRAME_SIZE) {
    throw new ProtocolError(
      `Payload too large: ${payloadLen} bytes (max: ${MAX_FRAME_SIZE})`,
    );
  }

  const frame = new Uint8Array(HEADER_SIZE + payloadLen);
  const view = new DataView(frame.buffer);

  // Header
  view.setUint32(0, payloadLen, false); // big-endian
  frame[4] = PROTOCOL_VERSION;
  frame[5] = msgType;
  frame[6] = format;

  // Payload
  frame.set(payloadBytes, HEADER_SIZE);

  return frame;
}

/**
 * Decoded frame result.
 */
export interface DecodedFrame {
  /** Message type constant */
  msgType: number;
  /** Serialization format */
  format: number;
  /** Deserialized payload */
  payload: unknown;
  /** Total bytes consumed from buffer */
  bytesConsumed: number;
}

/**
 * Try to decode a frame from a buffer.
 *
 * Returns null if the buffer doesn't contain a complete frame.
 * Throws ProtocolError if the frame is malformed.
 */
export function tryDecodeFrame(buffer: Uint8Array): DecodedFrame | null {
  if (buffer.length < HEADER_SIZE) {
    return null; // Not enough data for header
  }

  const view = new DataView(
    buffer.buffer,
    buffer.byteOffset,
    buffer.byteLength,
  );
  const payloadLen = view.getUint32(0, false); // big-endian

  if (payloadLen > MAX_FRAME_SIZE) {
    throw new ProtocolError(`Frame too large: ${payloadLen} bytes`);
  }

  const totalLen = HEADER_SIZE + payloadLen;

  if (buffer.length < totalLen) {
    return null; // Not enough data for full frame
  }

  const version = buffer[4];
  if (version !== PROTOCOL_VERSION) {
    throw new ProtocolError(
      `Unsupported protocol version: ${version} (expected ${PROTOCOL_VERSION})`,
    );
  }

  const msgType = buffer[5];
  const format = buffer[6];

  const payloadBytes = buffer.subarray(HEADER_SIZE, totalLen);
  let payload: unknown;

  if (format === FORMAT_MSGPACK) {
    payload = decode(payloadBytes);
  } else if (format === FORMAT_JSON) {
    const jsonStr = textDecoder.decode(payloadBytes);
    payload = JSON.parse(jsonStr);
  } else {
    throw new ProtocolError(`Unknown format: ${format}`);
  }

  return {
    msgType,
    format,
    payload,
    bytesConsumed: totalLen,
  };
}

/**
 * Generate a unique correlation ID.
 */
export function generateCorrelationId(prefix = "req"): string {
  const timestamp = Date.now().toString(16).padStart(12, "0");
  const random = Array.from(crypto.getRandomValues(new Uint8Array(4)))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `${prefix}_${timestamp}${random}`;
}
