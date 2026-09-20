/**
 * IPC protocol implementation matching acton-reactive.
 * @module
 */

import { decode, encode } from "@msgpack/msgpack";
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
/** Subscribe to IPC prefix patterns (requires engine 0.14.0 or later). */
export const MSG_TYPE_SUBSCRIBE_PATTERNS = 0x0a;
/** Unsubscribe from IPC prefix patterns (requires engine 0.14.0 or later). */
export const MSG_TYPE_UNSUBSCRIBE_PATTERNS = 0x0b;

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
 * Decode a frame body.
 *
 * An empty body decodes to `null`. A heartbeat frame is a bare header, and
 * neither JSON nor MessagePack can parse zero bytes.
 *
 * @throws {ProtocolError} If the format byte is unknown, or the body is not
 *   valid in its format, such as one cut short
 */
export function decodePayload(
  payloadBytes: Uint8Array,
  format: number,
): unknown {
  if (format !== FORMAT_MSGPACK && format !== FORMAT_JSON) {
    throw new ProtocolError(`Unknown format: ${format}`);
  }

  if (payloadBytes.length === 0) {
    return null;
  }

  // Both decoders throw their own errors, a SyntaxError from JSON and a
  // RangeError or DecodeError from MessagePack. Callers handle one class.
  try {
    if (format === FORMAT_JSON) {
      return JSON.parse(textDecoder.decode(payloadBytes));
    }
    return decode(payloadBytes);
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    const name = format === FORMAT_JSON ? "JSON" : "MessagePack";
    throw new ProtocolError(`Malformed ${name} frame body: ${reason}`);
  }
}

/** Names of the frame types this SDK knows, keyed by their wire byte. */
const FRAME_TYPE_NAMES: Readonly<Record<number, string>> = {
  [MSG_TYPE_REQUEST]: "REQUEST",
  [MSG_TYPE_RESPONSE]: "RESPONSE",
  [MSG_TYPE_ERROR]: "ERROR",
  [MSG_TYPE_HEARTBEAT]: "HEARTBEAT",
  [MSG_TYPE_PUSH]: "PUSH",
  [MSG_TYPE_SUBSCRIBE]: "SUBSCRIBE",
  [MSG_TYPE_UNSUBSCRIBE]: "UNSUBSCRIBE",
  [MSG_TYPE_DISCOVER]: "DISCOVER",
  [MSG_TYPE_STREAM]: "STREAM",
  [MSG_TYPE_SUBSCRIBE_PATTERNS]: "SUBSCRIBE_PATTERNS",
  [MSG_TYPE_UNSUBSCRIBE_PATTERNS]: "UNSUBSCRIBE_PATTERNS",
};

/**
 * Name a frame type byte for a log line.
 *
 * Returns `undefined` for a byte this SDK does not know.
 */
export function frameTypeName(msgType: number): string | undefined {
  return FRAME_TYPE_NAMES[msgType];
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
 * What the front of a read buffer holds.
 *
 * - `incomplete`: not a whole frame yet, so wait for more bytes
 * - `frame`: a decoded frame
 * - `bad-body`: a whole frame whose body does not decode. Its length is known,
 *   so the reader drops `bytesConsumed` bytes and carries on with the next
 *   frame
 * - `bad-framing`: a header that cannot be trusted, so the reader cannot tell
 *   where the next frame starts
 */
export type FrameStep =
  | { readonly kind: "incomplete" }
  | { readonly kind: "frame"; readonly frame: DecodedFrame }
  | {
    readonly kind: "bad-body";
    readonly msgType: number;
    readonly bytesConsumed: number;
    readonly reason: string;
  }
  | { readonly kind: "bad-framing"; readonly reason: string };

/**
 * Read the frame at the front of a buffer without throwing.
 *
 * A body that does not decode is told apart from a header that cannot be
 * trusted, because only the first leaves the reader able to find the next
 * frame.
 */
export function nextFrame(buffer: Uint8Array): FrameStep {
  if (buffer.length < HEADER_SIZE) {
    return { kind: "incomplete" };
  }

  const view = new DataView(
    buffer.buffer,
    buffer.byteOffset,
    buffer.byteLength,
  );
  const payloadLen = view.getUint32(0, false); // big-endian

  if (payloadLen > MAX_FRAME_SIZE) {
    return {
      kind: "bad-framing",
      reason: `Frame too large: ${payloadLen} bytes`,
    };
  }

  const version = buffer[4];
  if (version !== PROTOCOL_VERSION) {
    return {
      kind: "bad-framing",
      reason:
        `Unsupported protocol version: ${version} (expected ${PROTOCOL_VERSION})`,
    };
  }

  const totalLen = HEADER_SIZE + payloadLen;

  if (buffer.length < totalLen) {
    return { kind: "incomplete" };
  }

  const msgType = buffer[5];
  const format = buffer[6];

  try {
    const payload = decodePayload(
      buffer.subarray(HEADER_SIZE, totalLen),
      format,
    );
    return {
      kind: "frame",
      frame: { msgType, format, payload, bytesConsumed: totalLen },
    };
  } catch (err) {
    return {
      kind: "bad-body",
      msgType,
      bytesConsumed: totalLen,
      reason: err instanceof Error ? err.message : String(err),
    };
  }
}

/**
 * Try to decode a frame from a buffer.
 *
 * Returns null if the buffer doesn't contain a complete frame.
 * Throws ProtocolError if the frame is malformed.
 */
export function tryDecodeFrame(buffer: Uint8Array): DecodedFrame | null {
  const step = nextFrame(buffer);
  switch (step.kind) {
    case "incomplete":
      return null;
    case "frame":
      return step.frame;
    case "bad-body":
    case "bad-framing":
      throw new ProtocolError(step.reason);
  }
}

import { typeid } from "typeid-js";

/**
 * Generate a unique correlation ID in TypeID format.
 */
export function generateCorrelationId(prefix = "req"): string {
  return typeid(prefix).toString();
}
