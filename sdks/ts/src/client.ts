/**
 * Base client for socket connection management.
 * @module
 */

import {
  type ConnectOptions,
  type DiscoveryInfo,
  EmergentMessage,
  type IpcDiscoverResponse,
  type IpcEnvelope,
  type IpcPushNotification,
  type IpcResponse,
  type IpcSubscribeRequest,
  type PrimitiveKind,
  type TopologyPrimitive,
  type TopologyState,
  type WireMessage,
} from "./types.ts";
import {
  ConnectionError,
  DisposedError,
  ProtocolError,
  SocketNotFoundError,
  TimeoutError,
} from "./errors.ts";
import {
  encodeFrame,
  generateCorrelationId,
  HEADER_SIZE,
  MSG_TYPE_DISCOVER,
  MSG_TYPE_PUSH,
  MSG_TYPE_REQUEST,
  MSG_TYPE_RESPONSE,
  MSG_TYPE_SUBSCRIBE,
  MSG_TYPE_UNSUBSCRIBE,
  tryDecodeFrame,
} from "./protocol.ts";
import { MessageStream } from "./stream.ts";

// ============================================================================
// Platform Utilities
// ============================================================================

/**
 * Get the socket path from environment variable.
 *
 * The Emergent engine sets `EMERGENT_SOCKET` for managed processes.
 *
 * @throws {ConnectionError} If EMERGENT_SOCKET is not set
 */
export function getSocketPath(): string {
  // Deno runtime
  const socketPath = Deno.env.get("EMERGENT_SOCKET");

  if (!socketPath) {
    throw new ConnectionError(
      "EMERGENT_SOCKET environment variable not set. " +
        "Make sure the Emergent engine is running.",
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

// ============================================================================
// Pending Request Tracking
// ============================================================================

interface PendingRequest {
  resolve: (response: IpcResponse) => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
}

// ============================================================================
// Base Client
// ============================================================================

/** Default timeout for requests in milliseconds */
const DEFAULT_TIMEOUT_MS = 30000;

/**
 * Base client with shared connection logic.
 *
 * This class handles:
 * - Unix socket connection management
 * - Read loop with frame parsing
 * - Push notification handling (correctly extracting message from payload)
 * - Request/response correlation
 *
 * @internal
 */
export class BaseClient {
  protected conn: Deno.UnixConn | null = null;
  protected readonly name: string;
  protected readonly primitiveKind: PrimitiveKind;
  protected disposed = false;

  #readLoopRunning = false;
  #readBuffer: Uint8Array = new Uint8Array(0);
  #pendingRequests: Map<string, PendingRequest> = new Map();
  #messageStream: MessageStream | null = null;
  #subscribedTypes: Set<string> = new Set();
  #timeoutMs: number;

  constructor(name: string, kind: PrimitiveKind, options?: ConnectOptions) {
    this.name = name;
    this.primitiveKind = kind;
    this.#timeoutMs = options?.timeout ?? DEFAULT_TIMEOUT_MS;
  }

  /**
   * Get the list of currently subscribed message types.
   */
  subscribedTypes(): string[] {
    return Array.from(this.#subscribedTypes);
  }

  /**
   * Check if the client has been disposed.
   */
  get isDisposed(): boolean {
    return this.disposed;
  }

  /**
   * Connect to the socket.
   * @internal
   */
  protected async connectInternal(socketPath?: string): Promise<void> {
    if (this.disposed) {
      throw new DisposedError(this.constructor.name);
    }

    if (this.conn) {
      return; // Already connected
    }

    const path = socketPath ?? getSocketPath();

    // Check if socket exists
    if (!(await socketExists(path))) {
      throw new SocketNotFoundError(path);
    }

    try {
      this.conn = await Deno.connect({
        path,
        transport: "unix",
      });
      this.#startReadLoop();
    } catch (err) {
      if (err instanceof SocketNotFoundError) {
        throw err;
      }
      throw new ConnectionError(
        `Failed to connect to ${path}: ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
    }
  }

  /**
   * Subscribe to message types.
   *
   * The SDK automatically subscribes to `system.shutdown` and handles graceful
   * shutdown internally - when the engine signals shutdown for this primitive
   * kind, the stream will close gracefully.
   *
   * @internal
   */
  protected async subscribeInternal(
    messageTypes: string[],
  ): Promise<MessageStream> {
    this.#ensureConnected();

    const correlationId = generateCorrelationId("sub");

    // Create stream and register close callback
    const stream = new MessageStream(() => {
      // When stream closes, clear the message stream reference
      if (this.#messageStream === stream) {
        this.#messageStream = null;
      }
    });

    this.#messageStream = stream;

    // Add system.shutdown to subscriptions (SDK handles it internally)
    const allTypes = messageTypes.includes("system.shutdown")
      ? messageTypes
      : [...messageTypes, "system.shutdown"];

    const response = await this.#sendRequest<IpcSubscribeRequest>(
      MSG_TYPE_SUBSCRIBE,
      {
        correlation_id: correlationId,
        message_types: allTypes,
      },
      correlationId,
    );

    if (!response.success) {
      stream.close();
      throw new ConnectionError(response.error ?? "Subscription failed");
    }

    // Track subscribed types (exclude internal system.shutdown)
    for (const type of messageTypes) {
      if (type !== "system.shutdown") {
        this.#subscribedTypes.add(type);
      }
    }

    return stream;
  }

  /**
   * Unsubscribe from message types.
   * @internal
   */
  protected async unsubscribeInternal(messageTypes: string[]): Promise<void> {
    this.#ensureConnected();

    const correlationId = generateCorrelationId("unsub");

    const response = await this.#sendRequest<IpcSubscribeRequest>(
      MSG_TYPE_UNSUBSCRIBE,
      {
        correlation_id: correlationId,
        message_types: messageTypes,
      },
      correlationId,
    );

    if (!response.success) {
      // Log but don't fail - unsubscribe is best-effort
      console.warn("Unsubscribe warning:", response.error);
    }

    // Remove from tracked types
    for (const type of messageTypes) {
      this.#subscribedTypes.delete(type);
    }
  }

  /**
   * Publish a message.
   * @internal
   */
  protected async publishInternal(message: EmergentMessage): Promise<void> {
    this.#ensureConnected();

    // Convert to wire format and set source
    const wireMessage: WireMessage = {
      id: message.id,
      message_type: message.messageType,
      source: this.name, // Always use client name as source
      correlation_id: message.correlationId,
      causation_id: message.causationId,
      timestamp_ms: message.timestampMs,
      payload: message.payload,
      metadata: message.metadata,
    };

    // Wrap in IPC envelope (fire-and-forget, no reply expected)
    const envelope: IpcEnvelope = {
      correlation_id: generateCorrelationId("pub"),
      target: "broker",
      message_type: "Publish",
      payload: wireMessage,
      expects_reply: false,
    };

    const frame = encodeFrame(MSG_TYPE_REQUEST, envelope);
    await this.conn!.write(frame);
  }

  /**
   * Discover available message types and primitives.
   * @internal
   */
  protected async discoverInternal(): Promise<DiscoveryInfo> {
    this.#ensureConnected();

    const correlationId = generateCorrelationId("disc");

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target: "broker",
      message_type: "Discover",
      payload: null,
      expects_reply: true,
    };

    const response = await this.#sendRequest<IpcEnvelope>(
      MSG_TYPE_DISCOVER,
      envelope,
      correlationId,
    );

    if (!response.success) {
      throw new ConnectionError(response.error ?? "Discovery failed");
    }

    const discoverResponse = response.payload as IpcDiscoverResponse;

    return {
      messageTypes: Object.freeze([...discoverResponse.message_types]),
      primitives: Object.freeze(
        discoverResponse.primitives.map((p) => ({
          name: p.name,
          kind: p.kind as "Source" | "Handler" | "Sink",
        })),
      ),
    };
  }

  /**
   * Get the configured subscription types for this primitive.
   *
   * Queries the engine's config service to get the message types
   * this primitive should subscribe to based on the config.
   *
   * @internal
   */
  protected async getMySubscriptionsInternal(): Promise<string[]> {
    this.#ensureConnected();

    const correlationId = generateCorrelationId("getsub");

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target: "config_service",
      message_type: "GetSubscriptions",
      payload: { name: this.name },
      expects_reply: true,
    };

    const response = await this.#sendRequest<IpcEnvelope>(
      MSG_TYPE_REQUEST,
      envelope,
      correlationId,
    );

    if (!response.success) {
      throw new ConnectionError(response.error ?? "GetSubscriptions failed");
    }

    // Response payload has { subscribes: [...] }
    const payload = response.payload as { subscribes?: string[] } | null;
    return payload?.subscribes ?? [];
  }

  /**
   * Get the current topology (all primitives and their state).
   *
   * @internal
   */
  protected async getTopologyInternal(): Promise<TopologyState> {
    this.#ensureConnected();

    const correlationId = generateCorrelationId("gettopo");

    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target: "config_service",
      message_type: "GetTopology",
      payload: {},
      expects_reply: true,
    };

    const response = await this.#sendRequest<IpcEnvelope>(
      MSG_TYPE_REQUEST,
      envelope,
      correlationId,
    );

    if (!response.success) {
      throw new ConnectionError(response.error ?? "GetTopology failed");
    }

    // Response payload has { primitives: [...] }
    const payload = response.payload as { primitives?: TopologyPrimitive[] } | null;
    return { primitives: payload?.primitives ?? [] };
  }

  /**
   * Close the connection.
   */
  close(): void {
    if (this.disposed) return;

    this.#readLoopRunning = false;

    // Close message stream
    if (this.#messageStream) {
      this.#messageStream.close();
      this.#messageStream = null;
    }

    // Close connection
    if (this.conn) {
      try {
        this.conn.close();
      } catch {
        // Ignore close errors
      }
      this.conn = null;
    }

    // Cancel pending requests
    for (const [, pending] of this.#pendingRequests) {
      if (pending.timer) clearTimeout(pending.timer);
      pending.reject(new ConnectionError("Connection closed"));
    }
    this.#pendingRequests.clear();
    this.#subscribedTypes.clear();

    this.disposed = true;
  }

  /**
   * Async close with graceful cleanup.
   */
  disconnect(): Promise<void> {
    // Could add graceful shutdown logic here (e.g., wait for pending ops)
    this.close();
    return Promise.resolve();
  }

  // ============================================================================
  // Private Methods
  // ============================================================================

  #ensureConnected(): void {
    if (this.disposed) {
      throw new DisposedError(this.constructor.name);
    }
    if (!this.conn) {
      throw new ConnectionError("Not connected");
    }
  }

  #sendRequest<T>(
    msgType: number,
    payload: T,
    correlationId: string,
  ): Promise<IpcResponse> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pendingRequests.delete(correlationId);
        reject(new TimeoutError("Request timed out", this.#timeoutMs));
      }, this.#timeoutMs);

      this.#pendingRequests.set(correlationId, {
        resolve,
        reject,
        timer,
      });

      const frame = encodeFrame(msgType, payload);
      this.conn!.write(frame).catch((err) => {
        this.#pendingRequests.delete(correlationId);
        clearTimeout(timer);
        reject(
          new ConnectionError(
            `Failed to send: ${
              err instanceof Error ? err.message : String(err)
            }`,
          ),
        );
      });
    });
  }

  #startReadLoop(): void {
    if (this.#readLoopRunning || !this.conn) return;
    this.#readLoopRunning = true;

    // Start async read loop
    this.#runReadLoop().catch((err) => {
      if (this.#readLoopRunning) {
        console.error("Read loop error:", err);
        this.#messageStream?.closeWithError();
      }
    });
  }

  async #runReadLoop(): Promise<void> {
    const buffer = new Uint8Array(65536);

    try {
      while (this.#readLoopRunning && this.conn) {
        const n = await this.conn.read(buffer);
        if (n === null) {
          // EOF - connection closed
          break;
        }

        // Append to read buffer
        const newBuffer = new Uint8Array(this.#readBuffer.length + n);
        newBuffer.set(this.#readBuffer);
        newBuffer.set(buffer.subarray(0, n), this.#readBuffer.length);
        this.#readBuffer = newBuffer;

        // Process complete frames
        this.#processFrames();
      }
    } finally {
      this.#readLoopRunning = false;
    }
  }

  #processFrames(): void {
    while (this.#readBuffer.length >= HEADER_SIZE) {
      try {
        const result = tryDecodeFrame(this.#readBuffer);
        if (result === null) {
          // Not enough data for complete frame
          break;
        }

        // Consume the bytes
        this.#readBuffer = this.#readBuffer.subarray(result.bytesConsumed);

        // Handle the frame
        this.#handleFrame(result.msgType, result.payload);
      } catch (err) {
        if (err instanceof ProtocolError) {
          console.error("Protocol error:", err.message);
          // Reset buffer on protocol error
          this.#readBuffer = new Uint8Array(0);
          break;
        }
        throw err;
      }
    }
  }

  #handleFrame(msgType: number, payload: unknown): void {
    switch (msgType) {
      case MSG_TYPE_RESPONSE: {
        const response = payload as IpcResponse;
        const pending = this.#pendingRequests.get(response.correlation_id);
        if (pending) {
          this.#pendingRequests.delete(response.correlation_id);
          if (pending.timer) clearTimeout(pending.timer);
          pending.resolve(response);
        }
        break;
      }

      case MSG_TYPE_PUSH: {
        // CRITICAL FIX: The payload field contains the complete EmergentMessage
        // Do NOT generate new IDs - extract the original message from payload
        const notification = payload as IpcPushNotification;

        // Check for shutdown signal - SDK handles this internally
        if (notification.message_type === "system.shutdown") {
          const shutdownPayload = notification.payload as { kind?: string };
          const shutdownKind = shutdownPayload?.kind?.toLowerCase();

          // Close stream if shutdown is for this primitive's kind
          if (shutdownKind === this.primitiveKind.toLowerCase()) {
            // Graceful shutdown - close the stream
            if (this.#messageStream) {
              this.#messageStream.close();
              this.#messageStream = null;
            }
          }
          // Don't forward system.shutdown to user - it's internal
          break;
        }

        if (this.#messageStream) {
          // The notification.payload IS the serialized EmergentMessage (wire format)
          const wireMessage = notification.payload as WireMessage;

          // Convert from wire format (snake_case) to EmergentMessage class
          const message = EmergentMessage.fromWire(wireMessage);

          this.#messageStream.push(message);
        }
        break;
      }

      default:
        // Ignore unhandled message types
        break;
    }
  }
}
