/**
 * Base client for socket connection management.
 * @module
 */

import {
  type ConnectOptions,
  type DiscoveryInfo,
  EmergentMessage,
  type EmergentMessageData,
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
import { generateMessageId } from "./message.ts";
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
import { createLogger, type Logger } from "./logger.ts";

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

/** Pending pub/sub request with type-safe response */
interface PendingPubSubRequest<T> {
  resolve: (result: T) => void;
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

  #logger: Logger;
  #readLoopRunning = false;
  #readBuffer: Uint8Array = new Uint8Array(0);
  #pendingRequests: Map<string, PendingRequest> = new Map();
  #pendingTopologyRequests: Map<string, PendingPubSubRequest<TopologyState>> = new Map();
  #pendingSubscriptionsRequests: Map<string, PendingPubSubRequest<string[]>> = new Map();
  #messageStream: MessageStream | null = null;
  #subscribedTypes: Set<string> = new Set();
  #timeoutMs: number;

  constructor(name: string, kind: PrimitiveKind, options?: ConnectOptions) {
    this.name = name;
    this.primitiveKind = kind;
    this.#timeoutMs = options?.timeout ?? DEFAULT_TIMEOUT_MS;
    this.#logger = createLogger(name);
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

    this.#logger.info("connecting to engine", { kind: this.primitiveKind, path });

    // Check if socket exists
    if (!(await socketExists(path))) {
      this.#logger.error("engine socket not found", { path });
      throw new SocketNotFoundError(path);
    }

    try {
      this.conn = await Deno.connect({
        path,
        transport: "unix",
      });
      this.#startReadLoop();
      this.#logger.info("connected to engine", { kind: this.primitiveKind });
    } catch (err) {
      if (err instanceof SocketNotFoundError) {
        throw err;
      }
      const msg = err instanceof Error ? err.message : String(err);
      this.#logger.error("failed to connect to engine", { error: msg });
      throw new ConnectionError(
        `Failed to connect to ${path}: ${msg}`,
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

    this.#logger.info("subscribing to message types", { types: messageTypes });

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
      this.#logger.error("subscription failed", { types: messageTypes, error: response.error });
      stream.close();
      throw new ConnectionError(response.error ?? "Subscription failed");
    }

    // Track subscribed types (exclude internal system.shutdown)
    const subscribed: string[] = [];
    for (const type of messageTypes) {
      if (type !== "system.shutdown") {
        this.#subscribedTypes.add(type);
        subscribed.push(type);
      }
    }

    this.#logger.info("subscribed to message types", { types: subscribed });

    return stream;
  }

  /**
   * Unsubscribe from message types.
   * @internal
   */
  protected async unsubscribeInternal(messageTypes: string[]): Promise<void> {
    this.#ensureConnected();

    this.#logger.debug("unsubscribing from message types", { types: messageTypes });

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
      this.#logger.warn("unsubscribe failed", { types: messageTypes, error: response.error });
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

    this.#logger.debug("publishing message", { messageType: message.messageType, messageId: message.id });

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
    // Note: message_type must match the registered type name in the engine
    // Note: target must match the exposed actor name in the engine
    // Note: payload must be wrapped in {inner: ...} to match IpcEmergentMessage struct
    const envelope: IpcEnvelope = {
      correlation_id: generateCorrelationId("pub"),
      target: "message_broker", // Matches engine's runtime.ipc_expose("message_broker", ...)
      message_type: "EmergentMessage", // Matches engine's registry.register::<IpcEmergentMessage>("EmergentMessage")
      payload: { inner: wireMessage }, // Matches IpcEmergentMessage { inner: EmergentMessage }
      expects_reply: false,
    };

    const frame = encodeFrame(MSG_TYPE_REQUEST, envelope);
    try {
      await this.conn!.write(frame);
      this.#logger.debug("published message", { messageType: message.messageType, messageId: message.id });
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      this.#logger.error("failed to publish message", { messageType: message.messageType, messageId: message.id, error: errorMsg });
      throw err;
    }
  }

  /**
   * Publish a message with broker acknowledgment (backpressure).
   *
   * Unlike `publishInternal`, this waits for the engine's message broker to
   * confirm it has processed and forwarded the message before returning.
   *
   * @internal
   */
  protected async publishInternalAck(message: EmergentMessage): Promise<void> {
    this.#ensureConnected();

    this.#logger.debug("publishing message (ack)", { messageType: message.messageType, messageId: message.id });

    const wireMessage: WireMessage = {
      id: message.id,
      message_type: message.messageType,
      source: this.name,
      correlation_id: message.correlationId,
      causation_id: message.causationId,
      timestamp_ms: message.timestampMs,
      payload: message.payload,
      metadata: message.metadata,
    };

    const correlationId = generateCorrelationId("pub");
    const envelope: IpcEnvelope = {
      correlation_id: correlationId,
      target: "message_broker",
      message_type: "EmergentMessage",
      payload: { inner: wireMessage },
      expects_reply: true,
    };

    const response = await this.#sendRequest<IpcEnvelope>(
      MSG_TYPE_REQUEST,
      envelope,
      correlationId,
    );

    if (!response.success) {
      this.#logger.error("publish_ack failed", { messageType: message.messageType, error: response.error });
      throw new ConnectionError(response.error ?? "Broker returned error");
    }

    this.#logger.debug("publish_ack succeeded", { messageType: message.messageType, messageId: message.id });
  }

  /**
   * Discover available message types and primitives.
   * @internal
   */
  protected async discoverInternal(): Promise<DiscoveryInfo> {
    this.#ensureConnected();

    this.#logger.debug("sending discovery request");

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
      this.#logger.error("discovery failed", { error: response.error });
      throw new ConnectionError(response.error ?? "Discovery failed");
    }

    const discoverResponse = response.payload as IpcDiscoverResponse;

    this.#logger.debug("discovery complete", {
      messageTypes: discoverResponse.message_types.length,
      primitives: discoverResponse.primitives.length,
    });

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
   * Uses pub/sub pattern: publishes `system.request.subscriptions` and
   * waits for `system.response.subscriptions` with matching correlation_id.
   *
   * @internal
   */
  protected async getMySubscriptionsInternal(): Promise<string[]> {
    this.#ensureConnected();

    this.#logger.debug("querying configured subscriptions from engine");

    const correlationId = generateCorrelationId("cor");

    // Subscribe to response type first
    const subCorrelationId = generateCorrelationId("sub");
    const subResponse = await this.#sendRequest<IpcSubscribeRequest>(
      MSG_TYPE_SUBSCRIBE,
      {
        correlation_id: subCorrelationId,
        message_types: ["system.response.subscriptions"],
      },
      subCorrelationId,
    );

    if (!subResponse.success) {
      throw new ConnectionError(
        subResponse.error ?? "Failed to subscribe to response type"
      );
    }

    // Create promise to wait for response
    const resultPromise = new Promise<string[]>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pendingSubscriptionsRequests.delete(correlationId);
        reject(new TimeoutError("GetSubscriptions request timed out", this.#timeoutMs));
      }, this.#timeoutMs);

      this.#pendingSubscriptionsRequests.set(correlationId, {
        resolve,
        reject,
        timer,
      });
    });

    // Create and publish request message
    const requestData: EmergentMessageData = {
      id: generateMessageId(),
      messageType: "system.request.subscriptions",
      source: this.name,
      correlationId: correlationId,
      timestampMs: Date.now(),
      payload: { name: this.name },
    };
    const requestMessage = new EmergentMessage(requestData);
    await this.publishInternal(requestMessage);

    // Wait for response
    const result = await resultPromise;
    this.#logger.info("received configured subscriptions", { types: result });
    return result;
  }

  /**
   * Get the current topology (all primitives and their state).
   *
   * Uses pub/sub pattern: publishes `system.request.topology` and
   * waits for `system.response.topology` with matching correlation_id.
   *
   * @internal
   */
  protected async getTopologyInternal(): Promise<TopologyState> {
    this.#ensureConnected();

    this.#logger.debug("querying topology from engine");

    const correlationId = generateCorrelationId("cor");

    // Subscribe to response type first
    const subCorrelationId = generateCorrelationId("sub");
    const subResponse = await this.#sendRequest<IpcSubscribeRequest>(
      MSG_TYPE_SUBSCRIBE,
      {
        correlation_id: subCorrelationId,
        message_types: ["system.response.topology"],
      },
      subCorrelationId,
    );

    if (!subResponse.success) {
      throw new ConnectionError(
        subResponse.error ?? "Failed to subscribe to response type"
      );
    }

    // Create promise to wait for response
    const resultPromise = new Promise<TopologyState>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pendingTopologyRequests.delete(correlationId);
        reject(new TimeoutError("GetTopology request timed out", this.#timeoutMs));
      }, this.#timeoutMs);

      this.#pendingTopologyRequests.set(correlationId, {
        resolve,
        reject,
        timer,
      });
    });

    // Create and publish request message
    const requestData: EmergentMessageData = {
      id: generateMessageId(),
      messageType: "system.request.topology",
      source: this.name,
      correlationId: correlationId,
      timestampMs: Date.now(),
      payload: {},
    };
    const requestMessage = new EmergentMessage(requestData);
    await this.publishInternal(requestMessage);

    // Wait for response
    const result = await resultPromise;
    this.#logger.debug("received topology", { primitiveCount: result.primitives.length });
    return result;
  }

  /**
   * Close the connection.
   */
  close(): void {
    if (this.disposed) return;

    this.#logger.info("disconnecting from engine", { kind: this.primitiveKind });

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

    // Cancel pending topology requests
    for (const [, pending] of this.#pendingTopologyRequests) {
      if (pending.timer) clearTimeout(pending.timer);
      pending.reject(new ConnectionError("Connection closed"));
    }
    this.#pendingTopologyRequests.clear();

    // Cancel pending subscriptions requests
    for (const [, pending] of this.#pendingSubscriptionsRequests) {
      if (pending.timer) clearTimeout(pending.timer);
      pending.reject(new ConnectionError("Connection closed"));
    }
    this.#pendingSubscriptionsRequests.clear();

    this.#subscribedTypes.clear();

    this.#logger.info("disconnected from engine");

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
        const errorMsg = err instanceof Error ? err.message : String(err);
        this.#logger.error("failed to send request", { error: errorMsg });
        reject(
          new ConnectionError(
            `Failed to send: ${errorMsg}`,
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
        this.#logger.error("read loop error", { error: String(err) });
        this.#messageStream?.closeWithError();
      }
    });
  }

  async #runReadLoop(): Promise<void> {
    const buffer = new Uint8Array(65536);

    this.#logger.debug("read loop started");

    try {
      while (this.#readLoopRunning && this.conn) {
        const n = await this.conn.read(buffer);
        if (n === null) {
          // EOF - connection closed
          this.#logger.info("connection closed (EOF)");
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
          this.#logger.error("protocol error while processing frame", { error: err.message });
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
        // The payload field contains the complete EmergentMessage
        const notification = payload as IpcPushNotification;

        // Check for shutdown signal - SDK handles this internally
        if (notification.message_type === "system.shutdown") {
          const shutdownPayload = notification.payload as { kind?: string };
          const shutdownKind = shutdownPayload?.kind?.toLowerCase();

          this.#logger.info("received shutdown signal", { kind: shutdownKind });

          // Close stream if shutdown is for this primitive's kind
          if (shutdownKind === this.primitiveKind.toLowerCase()) {
            this.#logger.info("shutting down (engine requested)");
            // Graceful shutdown - close the stream
            if (this.#messageStream) {
              this.#messageStream.close();
              this.#messageStream = null;
            }
          }
          // Don't forward system.shutdown to user - it's internal
          break;
        }

        // Handle system.response.topology messages
        if (notification.message_type === "system.response.topology") {
          const wireMessage = notification.payload as WireMessage;
          const correlationId = wireMessage.correlation_id;
          if (correlationId) {
            const pending = this.#pendingTopologyRequests.get(correlationId);
            if (pending) {
              this.#pendingTopologyRequests.delete(correlationId);
              if (pending.timer) clearTimeout(pending.timer);
              // Extract primitives from payload
              const responsePayload = wireMessage.payload as { primitives?: TopologyPrimitive[] };
              pending.resolve({ primitives: responsePayload?.primitives ?? [] });
            }
          }
          break; // Don't forward to message stream
        }

        // Handle system.response.subscriptions messages
        if (notification.message_type === "system.response.subscriptions") {
          const wireMessage = notification.payload as WireMessage;
          const correlationId = wireMessage.correlation_id;
          if (correlationId) {
            const pending = this.#pendingSubscriptionsRequests.get(correlationId);
            if (pending) {
              this.#pendingSubscriptionsRequests.delete(correlationId);
              if (pending.timer) clearTimeout(pending.timer);
              // Extract subscribes from payload
              const responsePayload = wireMessage.payload as { subscribes?: string[] };
              pending.resolve(responsePayload?.subscribes ?? []);
            }
          }
          break; // Don't forward to message stream
        }

        if (this.#messageStream) {
          // The notification.payload IS the serialized EmergentMessage (wire format)
          const wireMessage = notification.payload as WireMessage;

          // Convert from wire format (snake_case) to EmergentMessage class
          const message = EmergentMessage.fromWire(wireMessage);

          this.#logger.debug("received message", { messageType: message.messageType, source: message.source });

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
