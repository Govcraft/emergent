/**
 * Emergent Client Library for Deno
 *
 * Provides type-safe primitives for building Emergent clients:
 * - EmergentSource: Publish messages (Sources can only publish)
 * - EmergentHandler: Subscribe and publish (Handlers do both)
 * - EmergentSink: Subscribe only (Sinks consume messages)
 *
 * @example
 * ```typescript
 * import { EmergentSink, EmergentMessage } from "./emergent_client.ts";
 *
 * const sink = await EmergentSink.connect("my_sink");
 * const stream = await sink.subscribe(["timer.tick", "timer.filtered"]);
 *
 * for await (const msg of stream) {
 *   console.log(`Received: ${msg.message_type}`, msg.payload);
 * }
 * ```
 *
 * @module
 */

// ============================================================================
// Protocol Constants (matching acton-reactive IPC)
// ============================================================================

const PROTOCOL_VERSION = 0x02;
const MAX_FRAME_SIZE = 16 * 1024 * 1024; // 16 MiB

// Message types
const MSG_TYPE_REQUEST = 0x01;
const MSG_TYPE_RESPONSE = 0x02;
const MSG_TYPE_PUSH = 0x05;
const MSG_TYPE_SUBSCRIBE = 0x06;
const MSG_TYPE_UNSUBSCRIBE = 0x07;

// Serialization formats
const FORMAT_JSON = 0x01;

// ============================================================================
// Error Classes
// ============================================================================

/** Error thrown when connection fails */
export class ConnectionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConnectionError";
  }
}

/** Error thrown when request times out */
export class TimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TimeoutError";
  }
}

/** Error thrown when server returns an error */
export class EmergentError extends Error {
  code: string;

  constructor(message: string, code: string = "UNKNOWN") {
    super(message);
    this.name = "EmergentError";
    this.code = code;
  }
}

// ============================================================================
// Types
// ============================================================================

/** Standard Emergent message envelope */
export interface EmergentMessage {
  /** Unique message ID */
  id: string;
  /** Message type for routing (e.g., "timer.tick") */
  message_type: string;
  /** Source client that published this message */
  source: string;
  /** Optional correlation ID for request-response patterns */
  correlation_id?: string;
  /** Optional causation ID (ID of message that triggered this one) */
  causation_id?: string;
  /** Timestamp when message was created (Unix ms) */
  timestamp_ms: number;
  /** User-defined payload */
  payload: unknown;
  /** Optional metadata */
  metadata?: unknown;
}

/** IPC response from server */
interface IpcResponse {
  correlation_id: string;
  success: boolean;
  error?: string;
  error_code?: string;
  payload?: unknown;
}

/** Push notification from server */
interface PushNotification {
  message_type: string;
  payload: unknown;
  timestamp_ms: number;
}

/** Subscription request */
interface SubscribeRequest {
  correlation_id: string;
  message_types: string[];
}

// ============================================================================
// Utility Functions
// ============================================================================

/**
 * Get the socket path from environment variable.
 * The Emergent engine sets EMERGENT_SOCKET for managed processes.
 */
export function getSocketPath(): string {
  const socketPath = Deno.env.get("EMERGENT_SOCKET");
  if (!socketPath) {
    throw new ConnectionError(
      "EMERGENT_SOCKET environment variable not set. " +
        "Make sure the Emergent engine is running."
    );
  }
  return socketPath;
}

/**
 * Check if a socket file exists.
 */
export async function socketExists(path: string): Promise<boolean> {
  try {
    await Deno.stat(path);
    return true;
  } catch {
    return false;
  }
}

/**
 * Generate a unique message ID (MTI-compatible format).
 */
function generateMessageId(prefix = "msg"): string {
  const timestamp = Date.now().toString(16).padStart(12, "0");
  const random = Array.from(crypto.getRandomValues(new Uint8Array(8)))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `${prefix}_${timestamp}${random}`;
}

// ============================================================================
// Frame Encoding/Decoding
// ============================================================================

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function encodeFrame(msgType: number, payload: unknown): Uint8Array {
  const jsonStr = JSON.stringify(payload);
  const payloadBytes = textEncoder.encode(jsonStr);

  // Frame structure: [length:4][version:1][msgType:1][format:1][payload:N]
  const payloadLen = payloadBytes.length;
  const frame = new Uint8Array(7 + payloadLen);
  const view = new DataView(frame.buffer);

  // Header
  view.setUint32(0, payloadLen, false); // big-endian
  frame[4] = PROTOCOL_VERSION;
  frame[5] = msgType;
  frame[6] = FORMAT_JSON;

  // Payload
  frame.set(payloadBytes, 7);

  return frame;
}

function decodePayload(bytes: Uint8Array): unknown {
  return JSON.parse(textDecoder.decode(bytes));
}

// ============================================================================
// Message Stream (AsyncIterable)
// ============================================================================

/**
 * Async iterator for receiving messages from subscriptions.
 */
export class MessageStream implements AsyncIterable<EmergentMessage> {
  private messageQueue: EmergentMessage[] = [];
  private waitingResolve: ((value: EmergentMessage | null) => void) | null =
    null;
  private closed = false;

  /** @internal */
  push(message: EmergentMessage): void {
    if (this.closed) return;

    if (this.waitingResolve) {
      this.waitingResolve(message);
      this.waitingResolve = null;
    } else {
      this.messageQueue.push(message);
    }
  }

  /** @internal */
  close(): void {
    this.closed = true;
    if (this.waitingResolve) {
      this.waitingResolve(null);
      this.waitingResolve = null;
    }
  }

  /**
   * Get the next message (for use with `while` loops).
   */
  async next(): Promise<EmergentMessage | null> {
    if (this.closed && this.messageQueue.length === 0) {
      return null;
    }

    if (this.messageQueue.length > 0) {
      return this.messageQueue.shift()!;
    }

    return new Promise((resolve) => {
      this.waitingResolve = resolve;
    });
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<EmergentMessage> {
    while (true) {
      const message = await this.next();
      if (message === null) break;
      yield message;
    }
  }
}

// ============================================================================
// Base Client (shared connection logic)
// ============================================================================

class BaseClient {
  protected conn: Deno.UnixConn | null = null;
  protected name: string;
  private readLoopRunning = false;
  private readBuffer: Uint8Array = new Uint8Array(0);
  private pendingRequests: Map<
    string,
    {
      resolve: (value: IpcResponse) => void;
      reject: (error: Error) => void;
      timer?: number;
    }
  > = new Map();
  private messageStream: MessageStream | null = null;

  constructor(name: string) {
    this.name = name;
  }

  protected async connectInternal(socketPath: string): Promise<void> {
    if (this.conn) {
      return;
    }

    try {
      this.conn = await Deno.connect({
        path: socketPath,
        transport: "unix",
      });
      this.startReadLoop();
    } catch (err) {
      throw new ConnectionError(`Failed to connect to ${socketPath}: ${err}`);
    }
  }

  protected async subscribeInternal(
    messageTypes: string[]
  ): Promise<MessageStream> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const correlationId = generateMessageId("sub");
    const stream = new MessageStream();
    this.messageStream = stream;

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(correlationId);
        reject(new TimeoutError("Subscription timed out"));
      }, 5000);

      this.pendingRequests.set(correlationId, {
        resolve: (response: IpcResponse) => {
          if (response.success) {
            resolve(stream);
          } else {
            reject(
              new EmergentError(response.error || "Subscription failed")
            );
          }
        },
        reject,
        timer,
      });

      const request: SubscribeRequest = {
        correlation_id: correlationId,
        message_types: messageTypes,
      };
      const frame = encodeFrame(MSG_TYPE_SUBSCRIBE, request);
      this.conn!.write(frame).catch(reject);
    });
  }

  protected async unsubscribeInternal(messageTypes: string[]): Promise<void> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    const correlationId = generateMessageId("unsub");

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(correlationId);
        resolve(); // Timeout, but don't fail
      }, 5000);

      this.pendingRequests.set(correlationId, {
        resolve: () => {
          resolve();
        },
        reject,
        timer,
      });

      const request: SubscribeRequest = {
        correlation_id: correlationId,
        message_types: messageTypes,
      };
      const frame = encodeFrame(MSG_TYPE_UNSUBSCRIBE, request);
      this.conn!.write(frame).catch(reject);
    });
  }

  protected async publishInternal(message: EmergentMessage): Promise<void> {
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }

    // Set the source to this client's name
    message.source = this.name;

    // Wrap in IPC envelope for fire-and-forget
    const envelope = {
      correlation_id: generateMessageId("pub"),
      target: "broker",
      message_type: "Publish",
      payload: message,
      expects_reply: false,
    };

    const frame = encodeFrame(MSG_TYPE_REQUEST, envelope);
    await this.conn.write(frame);
  }

  close(): void {
    this.readLoopRunning = false;
    if (this.messageStream) {
      this.messageStream.close();
    }
    if (this.conn) {
      try {
        this.conn.close();
      } catch {
        // Ignore close errors
      }
      this.conn = null;
    }

    for (const [, pending] of this.pendingRequests) {
      if (pending.timer) clearTimeout(pending.timer);
      pending.reject(new ConnectionError("Connection closed"));
    }
    this.pendingRequests.clear();
  }

  private async startReadLoop(): Promise<void> {
    if (this.readLoopRunning || !this.conn) return;
    this.readLoopRunning = true;

    const buffer = new Uint8Array(65536);

    try {
      while (this.readLoopRunning && this.conn) {
        const n = await this.conn.read(buffer);
        if (n === null) {
          break;
        }

        const newBuffer = new Uint8Array(this.readBuffer.length + n);
        newBuffer.set(this.readBuffer);
        newBuffer.set(buffer.subarray(0, n), this.readBuffer.length);
        this.readBuffer = newBuffer;

        this.processFrames();
      }
    } catch (err) {
      if (this.readLoopRunning) {
        console.error("Read loop error:", err);
      }
    } finally {
      this.readLoopRunning = false;
    }
  }

  private processFrames(): void {
    const HEADER_SIZE = 7;

    while (this.readBuffer.length >= HEADER_SIZE) {
      const lengthBytes = new Uint8Array(4);
      lengthBytes.set(this.readBuffer.subarray(0, 4));
      const view = new DataView(lengthBytes.buffer);
      const payloadLen = view.getUint32(0, false);

      if (payloadLen > MAX_FRAME_SIZE) {
        throw new EmergentError(`Frame too large: ${payloadLen}`);
      }

      const totalFrameLen = HEADER_SIZE + payloadLen;
      if (this.readBuffer.length < totalFrameLen) {
        break;
      }

      const msgType = this.readBuffer[5];
      const payloadBytes = new Uint8Array(payloadLen);
      payloadBytes.set(this.readBuffer.subarray(HEADER_SIZE, totalFrameLen));

      const remaining = this.readBuffer.length - totalFrameLen;
      if (remaining > 0) {
        const newBuf = new Uint8Array(remaining);
        newBuf.set(this.readBuffer.subarray(totalFrameLen));
        this.readBuffer = newBuf;
      } else {
        this.readBuffer = new Uint8Array(0);
      }

      try {
        const payload = decodePayload(payloadBytes);
        this.handleFrame(msgType, payload);
      } catch (err) {
        console.error("Failed to decode frame:", err);
      }
    }
  }

  private handleFrame(msgType: number, payload: unknown): void {
    switch (msgType) {
      case MSG_TYPE_RESPONSE: {
        const response = payload as IpcResponse;
        const pending = this.pendingRequests.get(response.correlation_id);
        if (pending) {
          this.pendingRequests.delete(response.correlation_id);
          if (pending.timer) clearTimeout(pending.timer);
          pending.resolve(response);
        }
        break;
      }

      case MSG_TYPE_PUSH: {
        const notification = payload as PushNotification;
        if (this.messageStream) {
          // Convert push notification to EmergentMessage
          const message: EmergentMessage = {
            id: generateMessageId("msg"),
            message_type: notification.message_type,
            source: "unknown",
            timestamp_ms: notification.timestamp_ms,
            payload: notification.payload,
          };

          // If the payload is itself an EmergentMessage envelope, use it directly
          const p = notification.payload as Record<string, unknown>;
          if (p && typeof p === "object" && "id" in p && "message_type" in p) {
            Object.assign(message, p);
          }

          this.messageStream.push(message);
        }
        break;
      }

      default:
        // Ignore other message types
        break;
    }
  }
}

// ============================================================================
// EmergentSource (publish only)
// ============================================================================

/**
 * A Source can only publish messages.
 * Use this for ingress components that bring data into the Emergent system.
 */
export class EmergentSource extends BaseClient {
  private constructor(name: string) {
    super(name);
  }

  /**
   * Connect to the Emergent engine as a Source.
   */
  static async connect(name: string): Promise<EmergentSource> {
    const source = new EmergentSource(name);
    const socketPath = getSocketPath();
    await source.connectInternal(socketPath);
    return source;
  }

  /**
   * Publish a message (fire-and-forget).
   */
  async publish(message: EmergentMessage): Promise<void> {
    await this.publishInternal(message);
  }
}

// ============================================================================
// EmergentHandler (subscribe + publish)
// ============================================================================

/**
 * A Handler can both subscribe to and publish messages.
 * Use this for processing components that transform or enrich data.
 */
export class EmergentHandler extends BaseClient {
  private constructor(name: string) {
    super(name);
  }

  /**
   * Connect to the Emergent engine as a Handler.
   */
  static async connect(name: string): Promise<EmergentHandler> {
    const handler = new EmergentHandler(name);
    const socketPath = getSocketPath();
    await handler.connectInternal(socketPath);
    return handler;
  }

  /**
   * Subscribe to message types and receive them via MessageStream.
   */
  async subscribe(messageTypes: string[]): Promise<MessageStream> {
    return await this.subscribeInternal(messageTypes);
  }

  /**
   * Unsubscribe from message types.
   */
  async unsubscribe(messageTypes: string[]): Promise<void> {
    await this.unsubscribeInternal(messageTypes);
  }

  /**
   * Publish a message (fire-and-forget).
   */
  async publish(message: EmergentMessage): Promise<void> {
    await this.publishInternal(message);
  }
}

// ============================================================================
// EmergentSink (subscribe only)
// ============================================================================

/**
 * A Sink can only subscribe to messages.
 * Use this for egress components that send data out of the Emergent system.
 */
export class EmergentSink extends BaseClient {
  private constructor(name: string) {
    super(name);
  }

  /**
   * Connect to the Emergent engine as a Sink.
   */
  static async connect(name: string): Promise<EmergentSink> {
    const sink = new EmergentSink(name);
    const socketPath = getSocketPath();
    await sink.connectInternal(socketPath);
    return sink;
  }

  /**
   * Subscribe to message types and receive them via MessageStream.
   */
  async subscribe(messageTypes: string[]): Promise<MessageStream> {
    return await this.subscribeInternal(messageTypes);
  }

  /**
   * Unsubscribe from message types.
   */
  async unsubscribe(messageTypes: string[]): Promise<void> {
    await this.unsubscribeInternal(messageTypes);
  }

  // Note: Sinks do NOT have a publish() method
}

// ============================================================================
// Message Builder
// ============================================================================

/**
 * Create a new EmergentMessage with builder pattern.
 */
export function createMessage(messageType: string): EmergentMessage {
  return {
    id: generateMessageId("msg"),
    message_type: messageType,
    source: "",
    timestamp_ms: Date.now(),
    payload: null,
  };
}

/**
 * Add payload to a message.
 */
export function withPayload<T>(
  message: EmergentMessage,
  payload: T
): EmergentMessage {
  return { ...message, payload };
}

/**
 * Add causation ID to a message (for tracing message chains).
 */
export function withCausationId(
  message: EmergentMessage,
  causationId: string
): EmergentMessage {
  return { ...message, causation_id: causationId };
}

/**
 * Add correlation ID to a message (for request-response patterns).
 */
export function withCorrelationId(
  message: EmergentMessage,
  correlationId: string
): EmergentMessage {
  return { ...message, correlation_id: correlationId };
}

/**
 * Add metadata to a message.
 */
export function withMetadata<T>(
  message: EmergentMessage,
  metadata: T
): EmergentMessage {
  return { ...message, metadata };
}
