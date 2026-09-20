/**
 * Base client for socket connection management.
 * @module
 */

import {
  type ConnectOptions,
  type DiscoveryInfo,
  EmergentMessage,
  type EmergentMessageData,
  type IpcDiscoverRequest,
  type IpcEnvelope,
  type IpcPatternSubscribeRequest,
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
  DiscoveryError,
  DisposedError,
  ProtocolError,
  PublishError,
  SocketNotFoundError,
  SubscriptionError,
  TimeoutError,
} from "./errors.ts";
import {
  encodeFrame,
  frameTypeName,
  generateCorrelationId,
  HEADER_SIZE,
  MSG_TYPE_DISCOVER,
  MSG_TYPE_ERROR,
  MSG_TYPE_HEARTBEAT,
  MSG_TYPE_PUSH,
  MSG_TYPE_REQUEST,
  MSG_TYPE_RESPONSE,
  MSG_TYPE_STREAM,
  MSG_TYPE_SUBSCRIBE,
  MSG_TYPE_SUBSCRIBE_PATTERNS,
  MSG_TYPE_UNSUBSCRIBE,
  MSG_TYPE_UNSUBSCRIBE_PATTERNS,
  nextFrame,
} from "./protocol.ts";
import { MessageStream } from "./stream.ts";
import { isEmergentMessageType, partitionTopics } from "./topics.ts";
import { createLogger, type Logger } from "./logger.ts";

// ============================================================================
// Platform Utilities
// ============================================================================

/**
 * Decide whether `EMERGENT_UNWRAP_STDOUT` switches stdout unwrapping on.
 *
 * Surrounding whitespace and letter case are ignored, and only `"true"` and
 * `"1"` enable it, the same rule as the Rust, Go and Python SDKs. Anything
 * else, including `"false"`, `"0"`, and an unset variable, leaves it off.
 */
export function parseUnwrapFlag(value: string | undefined): boolean {
  const normalized = value?.trim().toLowerCase();
  return normalized === "true" || normalized === "1";
}

/**
 * Read the primitive kind a `system.shutdown` broadcast targets.
 *
 * The engine forwards system events with the whole serialized message as the
 * notification payload, so the kind sits at `payload.kind` inside that
 * envelope. A bare `{"kind": ...}` object is accepted too, which keeps a
 * hand-written broadcast working. Returns `undefined` when no string kind is
 * present. The result is lowercased so callers can compare it against a
 * primitive kind directly.
 */
export function extractShutdownKind(
  notificationPayload: unknown,
): string | undefined {
  const kindOf = (value: unknown): string | undefined => {
    if (typeof value !== "object" || value === null) return undefined;
    const kind = (value as { kind?: unknown }).kind;
    return typeof kind === "string" ? kind.toLowerCase() : undefined;
  };

  if (typeof notificationPayload !== "object" || notificationPayload === null) {
    return undefined;
  }

  const inner = (notificationPayload as { payload?: unknown }).payload;
  return kindOf(inner) ?? kindOf(notificationPayload);
}

/** Error text for an ERROR frame that arrives without any of its own. */
export const DEFAULT_ERROR_TEXT = "Engine returned an error";

/**
 * Read the response a `RESPONSE` or `ERROR` frame carries.
 *
 * The engine answers a failed request with an `ERROR` frame whose body is the
 * same response object a `RESPONSE` frame carries: `correlation_id`,
 * `success`, `error` and `error_code`. An `ERROR` frame always reads as a
 * failure here, whatever its `success` field says, and always has error text.
 * Fields beside the shared ones are kept, because a discovery reply carries
 * its lists at the top level. Returns `undefined` for any other frame type,
 * for a body with no string `correlation_id`, since nothing can be matched to
 * it, and for a `RESPONSE` body with no boolean `success`.
 */
export function responseFromFrame(
  msgType: number,
  payload: unknown,
): IpcResponse | undefined {
  if (msgType !== MSG_TYPE_RESPONSE && msgType !== MSG_TYPE_ERROR) {
    return undefined;
  }
  if (
    typeof payload !== "object" || payload === null || Array.isArray(payload)
  ) {
    return undefined;
  }

  const body = payload as Record<string, unknown>;
  if (typeof body.correlation_id !== "string") return undefined;

  if (msgType === MSG_TYPE_ERROR) {
    const error = typeof body.error === "string" && body.error !== ""
      ? body.error
      : DEFAULT_ERROR_TEXT;
    return { ...body, success: false, error } as IpcResponse;
  }

  if (typeof body.success !== "boolean") return undefined;
  return body as unknown as IpcResponse;
}

/**
 * Describe an `ERROR` frame body for a log line.
 *
 * Gives the error text, followed by the error code in parentheses when the
 * engine sent one. Falls back to `DEFAULT_ERROR_TEXT` for a body that holds no
 * error text.
 */
export function errorFrameText(payload: unknown): string {
  if (
    typeof payload !== "object" || payload === null || Array.isArray(payload)
  ) {
    return DEFAULT_ERROR_TEXT;
  }
  const { error, error_code: code } = payload as Record<string, unknown>;
  const text = typeof error === "string" && error !== ""
    ? error
    : DEFAULT_ERROR_TEXT;
  return typeof code === "string" && code !== "" ? `${text} (${code})` : text;
}

/**
 * What the client does with a `RESPONSE` or `ERROR` frame.
 *
 * - `settle`: hand `response` to the request waiting on its correlation id
 * - `unmatched-error`: an `ERROR` nothing is waiting on, logged at error level
 * - `unmatched-response`: a `RESPONSE` nothing is waiting on, such as one that
 *   arrives after its request timed out
 * - `malformed`: a `RESPONSE` body that cannot be matched to anything
 */
export type ResponseSettlement =
  | { readonly kind: "settle"; readonly response: IpcResponse }
  | { readonly kind: "unmatched-error"; readonly text: string }
  | { readonly kind: "unmatched-response"; readonly correlationId: string }
  | { readonly kind: "malformed" };

/**
 * Decide what a `RESPONSE` or `ERROR` frame settles.
 *
 * An `ERROR` that matches no pending request is never dropped silently. The
 * engine sends one with the correlation id `unknown` for a request it could
 * not parse, and one with `__acton_connection_rejected__` when it refuses the
 * connection, and neither id belongs to a request.
 *
 * @param isPending - Whether a request is waiting on the given correlation id
 */
export function settlementFor(
  msgType: number,
  payload: unknown,
  isPending: (correlationId: string) => boolean,
): ResponseSettlement {
  const response = responseFromFrame(msgType, payload);

  if (response !== undefined && isPending(response.correlation_id)) {
    return { kind: "settle", response };
  }
  if (msgType === MSG_TYPE_ERROR) {
    return { kind: "unmatched-error", text: errorFrameText(payload) };
  }
  if (response === undefined) {
    return { kind: "malformed" };
  }
  return {
    kind: "unmatched-response",
    correlationId: response.correlation_id,
  };
}

/**
 * Build `DiscoveryInfo` from a successful discovery response.
 *
 * The engine writes `actors` and `message_types` at the top level of the
 * response, and leaves out whichever list was not asked for. Each actor
 * becomes a `PrimitiveInfo` with no kind, because the response does not carry
 * one. Entries of the wrong shape are dropped.
 *
 * @throws {ProtocolError} If the response is not an object, or either list is
 *   present and not an array
 */
export function discoveryInfoFromResponse(response: unknown): DiscoveryInfo {
  if (
    typeof response !== "object" || response === null ||
    Array.isArray(response)
  ) {
    throw new ProtocolError("Malformed discovery response: not an object");
  }

  const listOf = (field: string): unknown[] => {
    const value = (response as Record<string, unknown>)[field];
    if (value === undefined || value === null) return [];
    if (!Array.isArray(value)) {
      throw new ProtocolError(
        `Malformed discovery response: ${field} is not a list`,
      );
    }
    return value;
  };

  const messageTypes = listOf("message_types").filter(
    (entry): entry is string => typeof entry === "string",
  );
  const primitives = listOf("actors").flatMap((actor) => {
    if (typeof actor !== "object" || actor === null) return [];
    const name = (actor as { name?: unknown }).name;
    return typeof name === "string" ? [Object.freeze({ name })] : [];
  });

  return {
    messageTypes: Object.freeze(messageTypes),
    primitives: Object.freeze(primitives),
  };
}

/**
 * Name a frame type for a log line, falling back to its byte in hex.
 */
export function frameTypeLabel(msgType: number): string {
  return frameTypeName(msgType) ??
    `0x${msgType.toString(16).padStart(2, "0")}`;
}

/**
 * Read the notification a `PUSH` frame carries.
 *
 * Only `message_type` is needed to route a notification, so it is the only
 * field checked. Returns `undefined` for a body that is not an object or has
 * no string `message_type`.
 */
export function pushFromFrame(
  payload: unknown,
): IpcPushNotification | undefined {
  if (
    typeof payload !== "object" || payload === null || Array.isArray(payload)
  ) {
    return undefined;
  }
  const body = payload as Record<string, unknown>;
  if (typeof body.message_type !== "string") return undefined;
  return body as unknown as IpcPushNotification;
}

/**
 * Read the Emergent message a push notification carries as its payload.
 *
 * `id`, `message_type` and `source` must be strings and `timestamp_ms` a
 * number. `correlation_id` and `causation_id` must be strings when present. A
 * `null` there reads as absent, which is how a publisher that does not omit
 * empty fields writes one. `payload` and `metadata` are kept as sent. Returns
 * `undefined` for anything else, so a message of the wrong shape never
 * reaches the subscriber.
 */
export function wireMessageFromPush(payload: unknown): WireMessage | undefined {
  if (
    typeof payload !== "object" || payload === null || Array.isArray(payload)
  ) {
    return undefined;
  }
  const body = payload as Record<string, unknown>;

  if (
    typeof body.id !== "string" || typeof body.message_type !== "string" ||
    typeof body.source !== "string" || typeof body.timestamp_ms !== "number"
  ) {
    return undefined;
  }

  const isOptionalString = (
    value: unknown,
  ): value is string | null | undefined =>
    value === undefined || value === null || typeof value === "string";
  const { correlation_id: correlationId, causation_id: causationId } = body;
  if (!isOptionalString(correlationId) || !isOptionalString(causationId)) {
    return undefined;
  }

  return {
    id: body.id,
    message_type: body.message_type,
    source: body.source,
    correlation_id: correlationId ?? undefined,
    causation_id: causationId ?? undefined,
    timestamp_ms: body.timestamp_ms,
    payload: body.payload,
    metadata: body.metadata ?? undefined,
  };
}

/**
 * Read the primitives a `system.response.topology` message payload lists.
 *
 * Entries that are not objects with a string `name` are dropped. A payload
 * with no `primitives` list reads as an empty topology.
 */
export function topologyFromPayload(payload: unknown): TopologyState {
  if (typeof payload !== "object" || payload === null) {
    return { primitives: [] };
  }
  const { primitives } = payload as { primitives?: unknown };
  if (!Array.isArray(primitives)) return { primitives: [] };
  return {
    primitives: primitives.filter((entry): entry is TopologyPrimitive =>
      typeof entry === "object" && entry !== null &&
      typeof (entry as { name?: unknown }).name === "string"
    ),
  };
}

/**
 * Read the topics a `system.response.subscriptions` message payload lists.
 *
 * Entries that are not strings are dropped. A payload with no `subscribes`
 * list reads as no subscriptions.
 */
export function subscribesFromPayload(payload: unknown): string[] {
  if (typeof payload !== "object" || payload === null) return [];
  const { subscribes } = payload as { subscribes?: unknown };
  if (!Array.isArray(subscribes)) return [];
  return subscribes.filter((entry): entry is string =>
    typeof entry === "string"
  );
}

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

/** The part of a connection a frame is written through. */
export interface FrameWriter {
  write(bytes: Uint8Array): Promise<number>;
}

/**
 * Write every byte of `bytes`.
 *
 * `Deno.Conn.write` resolves to the number of bytes it wrote, which can be
 * fewer than it was given when the socket buffer is full. Half a frame on the
 * wire costs the engine its framing for the rest of the connection.
 *
 * @throws {ConnectionError} If the writer takes no bytes, which would loop
 *   forever
 */
export async function writeAll(
  writer: FrameWriter,
  bytes: Uint8Array,
): Promise<void> {
  let written = 0;
  while (written < bytes.length) {
    const n = await writer.write(bytes.subarray(written));
    if (n <= 0) {
      throw new ConnectionError("Socket accepted no bytes");
    }
    written += n;
  }
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
  #pendingTopologyRequests: Map<string, PendingPubSubRequest<TopologyState>> =
    new Map();
  #pendingSubscriptionsRequests: Map<string, PendingPubSubRequest<string[]>> =
    new Map();
  #messageStream: MessageStream | null = null;
  #subscribedTypes: Set<string> = new Set();
  #timeoutMs: number;
  /** Settles when the last queued frame is out, whether or not it failed. */
  #writeTail: Promise<void> = Promise.resolve();
  /** Frames written or waiting to be. Zero means the next one need not wait. */
  #queuedWrites = 0;

  #unwrapStdout: boolean;

  constructor(name: string, kind: PrimitiveKind, options?: ConnectOptions) {
    this.name = name;
    this.primitiveKind = kind;
    this.#timeoutMs = options?.timeout ?? DEFAULT_TIMEOUT_MS;
    this.#logger = createLogger(name);
    this.#unwrapStdout = parseUnwrapFlag(
      Deno.env.get("EMERGENT_UNWRAP_STDOUT"),
    );
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

    this.#logger.info("connecting to engine", {
      kind: this.primitiveKind,
      path,
    });

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
      this.startReadLoop();
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

    // Split before anything is sent, so a topic that could never match, such
    // as "system.*.error", throws here instead of subscribing to silence.
    const { exact, patterns } = partitionTopics(messageTypes);

    this.#logger.info("subscribing to message types", {
      types: exact,
      patterns,
    });

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
    const allTypes = exact.includes("system.shutdown")
      ? exact
      : [...exact, "system.shutdown"];

    const response = await this.#sendRequest<IpcSubscribeRequest>(
      MSG_TYPE_SUBSCRIBE,
      {
        correlation_id: correlationId,
        message_types: allTypes,
      },
      correlationId,
    );

    if (!response.success) {
      this.#logger.error("subscription failed", {
        types: messageTypes,
        error: response.error,
      });
      stream.close();
      throw new SubscriptionError(
        response.error ?? "Subscription failed",
        exact,
      );
    }

    if (patterns.length > 0) {
      // Patterns travel on their own request because the engine keeps a
      // separate index for them. A connection matching a message through both
      // indexes still receives one copy.
      const patternCorrelationId = generateCorrelationId("psub");
      const patternResponse = await this.#sendRequest<
        IpcPatternSubscribeRequest
      >(
        MSG_TYPE_SUBSCRIBE_PATTERNS,
        {
          correlation_id: patternCorrelationId,
          patterns,
        },
        patternCorrelationId,
      );

      if (!patternResponse.success) {
        this.#logger.error("pattern subscription failed", {
          patterns,
          error: patternResponse.error,
        });
        stream.close();
        throw new SubscriptionError(
          patternResponse.error ?? "Pattern subscription failed",
          patterns,
        );
      }
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

    this.#logger.debug("unsubscribing from message types", {
      types: messageTypes,
    });

    const { exact, patterns } = partitionTopics(messageTypes);

    const correlationId = generateCorrelationId("unsub");

    const response = await this.#sendRequest<IpcSubscribeRequest>(
      MSG_TYPE_UNSUBSCRIBE,
      {
        correlation_id: correlationId,
        message_types: exact,
      },
      correlationId,
    );

    if (!response.success) {
      // Log but don't fail - unsubscribe is best-effort
      this.#logger.warn("unsubscribe failed", {
        types: exact,
        error: response.error,
      });
    }

    if (patterns.length > 0) {
      const patternCorrelationId = generateCorrelationId("punsub");
      const patternResponse = await this.#sendRequest<
        IpcPatternSubscribeRequest
      >(
        MSG_TYPE_UNSUBSCRIBE_PATTERNS,
        {
          correlation_id: patternCorrelationId,
          patterns,
        },
        patternCorrelationId,
      );

      if (!patternResponse.success) {
        this.#logger.warn("pattern unsubscribe failed", {
          patterns,
          error: patternResponse.error,
        });
      }
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

    this.#logger.debug("publishing message", {
      messageType: message.messageType,
      messageId: message.id,
    });

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
      await this.#writeFrame(frame);
      this.#logger.debug("published message", {
        messageType: message.messageType,
        messageId: message.id,
      });
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      this.#logger.error("failed to publish message", {
        messageType: message.messageType,
        messageId: message.id,
        error: errorMsg,
      });
      throw new PublishError(
        `Failed to publish: ${errorMsg}`,
        message.messageType,
        { cause: err },
      );
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

    this.#logger.debug("publishing message (ack)", {
      messageType: message.messageType,
      messageId: message.id,
    });

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
      this.#logger.error("publish_ack failed", {
        messageType: message.messageType,
        error: response.error,
      });
      throw new PublishError(
        response.error ?? "Broker returned error",
        message.messageType,
      );
    }

    this.#logger.debug("publish_ack succeeded", {
      messageType: message.messageType,
      messageId: message.id,
    });
  }

  /**
   * Ask the engine's IPC layer what it exposes: its registered IPC type names
   * and its IPC-exposed actors. Neither list holds Emergent topics or
   * primitives, which `getTopologyInternal` reports.
   * @internal
   */
  protected async discoverInternal(): Promise<DiscoveryInfo> {
    this.#ensureConnected();

    this.#logger.debug("sending discovery request");

    const correlationId = generateCorrelationId("disc");

    const response = await this.#sendRequest<IpcDiscoverRequest>(
      MSG_TYPE_DISCOVER,
      {
        correlation_id: correlationId,
        include_actors: true,
        include_message_types: true,
      },
      correlationId,
    );

    if (!response.success) {
      this.#logger.error("discovery failed", { error: response.error });
      throw new DiscoveryError(response.error ?? "Discovery failed");
    }

    const info = discoveryInfoFromResponse(response);

    this.#logger.debug("discovery complete", {
      messageTypes: info.messageTypes.length,
      primitives: info.primitives.length,
    });

    return info;
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
      throw new SubscriptionError(
        subResponse.error ?? "Failed to subscribe to response type",
        ["system.response.subscriptions"],
      );
    }

    // Create promise to wait for response
    const resultPromise = new Promise<string[]>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pendingSubscriptionsRequests.delete(correlationId);
        reject(
          new TimeoutError(
            "GetSubscriptions request timed out",
            this.#timeoutMs,
          ),
        );
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
    const result = await this.#publishQuery(
      this.#pendingSubscriptionsRequests,
      new EmergentMessage(requestData),
      resultPromise,
    );
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
      throw new SubscriptionError(
        subResponse.error ?? "Failed to subscribe to response type",
        ["system.response.topology"],
      );
    }

    // Create promise to wait for response
    const resultPromise = new Promise<TopologyState>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pendingTopologyRequests.delete(correlationId);
        reject(
          new TimeoutError("GetTopology request timed out", this.#timeoutMs),
        );
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
    const result = await this.#publishQuery(
      this.#pendingTopologyRequests,
      new EmergentMessage(requestData),
      resultPromise,
    );
    this.#logger.debug("received topology", {
      primitiveCount: result.primitives.length,
    });
    return result;
  }

  /**
   * Close the connection.
   */
  close(): void {
    if (this.disposed) return;

    this.#logger.info("disconnecting from engine", {
      kind: this.primitiveKind,
    });

    this.#readLoopRunning = false;

    // Close connection
    if (this.conn) {
      try {
        this.conn.close();
      } catch {
        // Ignore close errors
      }
      this.conn = null;
    }

    this.#failEverythingPending();

    this.#subscribedTypes.clear();

    this.#logger.info("disconnected from engine");

    this.disposed = true;
  }

  /**
   * End the message stream and fail every request still in flight.
   *
   * For a connection that is gone, whether the caller closed it or the engine
   * did. Nothing pending can be answered any more, so each request and query
   * fails with a ConnectionError now and does not wait out its timer, and a
   * `for await` over the stream stops.
   */
  #failEverythingPending(): void {
    if (this.#messageStream) {
      this.#messageStream.close();
      this.#messageStream = null;
    }

    const inFlight: Map<
      string,
      { reject: (error: Error) => void; timer?: ReturnType<typeof setTimeout> }
    >[] = [
      this.#pendingRequests,
      this.#pendingTopologyRequests,
      this.#pendingSubscriptionsRequests,
    ];
    for (const requests of inFlight) {
      for (const pending of requests.values()) {
        if (pending.timer) clearTimeout(pending.timer);
        pending.reject(new ConnectionError("Connection closed"));
      }
      requests.clear();
    }
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

  /**
   * Write one whole frame, after every frame queued before it.
   *
   * Callers do not wait for each other, so without the queue a second frame
   * could land between two slices of a partly written first one. With nothing
   * queued the write starts at once, as it did before there was a queue.
   */
  #writeFrame(frame: Uint8Array): Promise<void> {
    const send = async (): Promise<void> => {
      // The connection may have closed while this frame waited its turn.
      if (!this.conn) {
        throw new ConnectionError("Not connected");
      }
      await writeAll(this.conn, frame);
    };
    const written = this.#queuedWrites === 0
      ? send()
      : this.#writeTail.then(send);
    this.#queuedWrites++;
    this.#writeTail = written.catch(() => {}).finally(() => {
      this.#queuedWrites--;
    });
    return written;
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
      this.#writeFrame(frame).catch((err) => {
        this.#pendingRequests.delete(correlationId);
        clearTimeout(timer);
        const errorMsg = err instanceof Error ? err.message : String(err);
        this.#logger.error("failed to send request", { error: errorMsg });
        reject(
          new ConnectionError(
            `Failed to send: ${errorMsg}`,
            "CONNECTION_FAILED",
            { cause: err },
          ),
        );
      });
    });
  }

  /**
   * Start reading the connection.
   *
   * @internal
   */
  protected startReadLoop(): void {
    if (this.#readLoopRunning || !this.conn) return;
    this.#readLoopRunning = true;

    // Start async read loop
    this.#runReadLoop()
      .catch((err) => {
        // close() ends the read under the loop; that is not an error.
        if (!this.disposed) {
          this.#logger.error("read loop error", { error: String(err) });
        }
      })
      .then(() => {
        // The engine is gone, so nothing in flight can be answered. After
        // close() there is nothing left to fail.
        if (!this.disposed) this.#failEverythingPending();
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

        this.receiveBytes(buffer.subarray(0, n));
      }
    } finally {
      this.#readLoopRunning = false;
    }
  }

  /**
   * Take bytes read from the socket and handle every complete frame in them.
   *
   * @internal
   */
  protected receiveBytes(chunk: Uint8Array): void {
    // Append to read buffer
    const newBuffer = new Uint8Array(this.#readBuffer.length + chunk.length);
    newBuffer.set(this.#readBuffer);
    newBuffer.set(chunk, this.#readBuffer.length);
    this.#readBuffer = newBuffer;

    // Process complete frames
    this.#processFrames();
  }

  #processFrames(): void {
    while (this.#readBuffer.length >= HEADER_SIZE) {
      const step = nextFrame(this.#readBuffer);

      if (step.kind === "incomplete") break;

      if (step.kind === "bad-framing") {
        // Nothing says where the next frame starts, so drop what is buffered.
        this.#logger.error("protocol error while processing frame", {
          error: step.reason,
        });
        this.#readBuffer = new Uint8Array(0);
        break;
      }

      if (step.kind === "bad-body") {
        this.#logger.warn("skipping frame with malformed body", {
          msgType: frameTypeLabel(step.msgType),
          error: step.reason,
        });
        this.#readBuffer = this.#readBuffer.subarray(step.bytesConsumed);
        continue;
      }

      this.#readBuffer = this.#readBuffer.subarray(step.frame.bytesConsumed);

      // One frame must never end the read loop, whatever handling it throws.
      try {
        this.#handleFrame(step.frame.msgType, step.frame.payload);
      } catch (err) {
        this.#logger.error("skipping frame that could not be handled", {
          msgType: frameTypeLabel(step.frame.msgType),
          error: err instanceof Error ? err.message : String(err),
        });
      }
    }
  }

  /**
   * Apply a `system.shutdown` push notification to the subscriber stream.
   *
   * Returns the stream to keep: the same one when the broadcast targets
   * another primitive kind, or `null` after closing it when the broadcast
   * targets this one.
   *
   * @internal
   */
  protected applyShutdownNotification(
    notificationPayload: unknown,
    stream: MessageStream | null,
  ): MessageStream | null {
    const shutdownKind = extractShutdownKind(notificationPayload);

    this.#logger.info("received shutdown signal", {
      kind: shutdownKind ?? "unknown",
    });

    if (shutdownKind !== this.primitiveKind.toLowerCase()) {
      return stream;
    }

    this.#logger.info("shutting down (engine requested)");
    stream?.close();
    return null;
  }

  /**
   * Settle the pending request a RESPONSE or ERROR frame answers.
   *
   * A failed request comes back as an ERROR frame. It settles the pending
   * request like any response, with `success` false, and the caller throws its
   * own error from the engine's error text.
   */
  #settleResponse(msgType: number, payload: unknown): void {
    const settlement = settlementFor(
      msgType,
      payload,
      (correlationId) => this.#pendingRequests.has(correlationId),
    );

    switch (settlement.kind) {
      case "settle": {
        const { response } = settlement;
        const pending = this.#pendingRequests.get(response.correlation_id);
        this.#pendingRequests.delete(response.correlation_id);
        if (pending?.timer) clearTimeout(pending.timer);
        pending?.resolve(response);
        break;
      }
      case "unmatched-error":
        // No request to fail, for example a connection-level rejection or a
        // request the engine could not parse.
        this.#logger.error("engine error matched no pending request", {
          error: settlement.text,
        });
        break;
      case "unmatched-response":
        this.#logger.debug("response matched no pending request", {
          correlationId: settlement.correlationId,
        });
        break;
      case "malformed":
        this.#logger.warn("dropping malformed response frame");
        break;
    }
  }

  /**
   * Publish a pub/sub query's request and wait for its answer.
   *
   * A refused publish takes the pending entry and its timer with it. Left
   * armed, the timer would reject a promise nobody holds.
   */
  async #publishQuery<T>(
    requests: Map<string, PendingPubSubRequest<T>>,
    request: EmergentMessage,
    answer: Promise<T>,
  ): Promise<T> {
    try {
      await this.publishInternal(request);
    } catch (error) {
      this.#takePending(requests, request.correlationId);
      throw error;
    }
    return await answer;
  }

  /** Remove and return the pub/sub request waiting on a correlation id. */
  #takePending<T>(
    requests: Map<string, PendingPubSubRequest<T>>,
    correlationId: string | undefined,
  ): PendingPubSubRequest<T> | undefined {
    if (correlationId === undefined) return undefined;
    const pending = requests.get(correlationId);
    if (pending === undefined) return undefined;
    requests.delete(correlationId);
    if (pending.timer) clearTimeout(pending.timer);
    return pending;
  }

  /**
   * Route a PUSH frame: to the SDK's own handling for the system messages it
   * owns, and to the subscriber stream for everything else. A notification or
   * message of the wrong shape is logged and dropped.
   */
  #handlePush(payload: unknown): void {
    const notification = pushFromFrame(payload);
    if (notification === undefined) {
      this.#logger.warn("dropping malformed push frame");
      return;
    }
    const messageType = notification.message_type;

    // Check for shutdown signal - SDK handles this internally
    if (messageType === "system.shutdown") {
      this.#messageStream = this.applyShutdownNotification(
        notification.payload,
        this.#messageStream,
      );
      // Don't forward system.shutdown to user - it's internal
      return;
    }

    // Skip the transport's own envelope broadcasts, which only a "*"
    // subscription ever sees. The message inside each one arrives
    // separately under its own Emergent message type.
    if (!isEmergentMessageType(messageType)) {
      this.#logger.debug("skipping non-Emergent IPC broadcast", {
        messageType,
      });
      return;
    }

    // The notification.payload IS the serialized EmergentMessage (wire format)
    const wireMessage = wireMessageFromPush(notification.payload);
    if (wireMessage === undefined) {
      this.#logger.warn("dropping push with a malformed message", {
        messageType,
      });
      return;
    }

    // Handle system.response.topology messages
    if (messageType === "system.response.topology") {
      const pending = this.#takePending(
        this.#pendingTopologyRequests,
        wireMessage.correlation_id,
      );
      pending?.resolve(topologyFromPayload(wireMessage.payload));
      return; // Don't forward to message stream
    }

    // Handle system.response.subscriptions messages
    if (messageType === "system.response.subscriptions") {
      const pending = this.#takePending(
        this.#pendingSubscriptionsRequests,
        wireMessage.correlation_id,
      );
      pending?.resolve(subscribesFromPayload(wireMessage.payload));
      return; // Don't forward to message stream
    }

    if (!this.#messageStream) return;

    // Convert from wire format (snake_case) to EmergentMessage class
    let message = EmergentMessage.fromWire(wireMessage);

    // Auto-unwrap stdout payloads when enabled (skip system messages)
    if (this.#unwrapStdout && !message.messageType.startsWith("system.")) {
      message = message.unwrapStdout();
    }

    this.#logger.debug("received message", {
      messageType: message.messageType,
      source: message.source,
    });

    this.#messageStream.push(message);
  }

  #handleFrame(msgType: number, payload: unknown): void {
    switch (msgType) {
      case MSG_TYPE_RESPONSE:
      case MSG_TYPE_ERROR:
        this.#settleResponse(msgType, payload);
        break;

      case MSG_TYPE_HEARTBEAT:
      case MSG_TYPE_STREAM:
        // The engine echoes a heartbeat only after receiving one, and streams
        // only to a request that asked for a stream. This SDK sends neither,
        // so there is nothing to route.
        this.#logger.debug("ignoring frame", {
          msgType: frameTypeName(msgType),
        });
        break;

      case MSG_TYPE_PUSH:
        this.#handlePush(payload);
        break;

      default:
        this.#logger.warn("ignoring frame of unexpected type", {
          msgType: frameTypeLabel(msgType),
        });
        break;
    }
  }
}
