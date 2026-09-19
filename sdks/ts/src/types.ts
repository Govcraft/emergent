/**
 * Core types for the Emergent client SDK.
 * @module
 */

// ============================================================================
// Public Types
// ============================================================================

/**
 * Standard Emergent message envelope.
 *
 * Messages are immutable once created. Use {@link MessageBuilder} to create
 * new messages with modified fields.
 */
export class EmergentMessage {
  /** Unique message ID (MTI format: msg_<uuid_v7>) */
  readonly id: string;
  /** Message type for routing (e.g., "timer.tick") */
  readonly messageType: string;
  /** Source client that published this message */
  readonly source: string;
  /** Optional correlation ID for request-response patterns */
  readonly correlationId?: string;
  /** Optional causation ID (ID of message that triggered this one) */
  readonly causationId?: string;
  /** Timestamp when message was created (Unix ms) */
  readonly timestampMs: number;
  /** User-defined payload */
  readonly payload: unknown;
  /** Optional metadata for tracing/debugging */
  readonly metadata?: Record<string, unknown>;

  /** @internal */
  constructor(data: EmergentMessageData) {
    this.id = data.id;
    this.messageType = data.messageType;
    this.source = data.source;
    this.correlationId = data.correlationId;
    this.causationId = data.causationId;
    this.timestampMs = data.timestampMs;
    this.payload = data.payload;
    this.metadata = data.metadata;
    Object.freeze(this);
  }

  /**
   * Get the payload as a specific type.
   *
   * @example
   * ```typescript
   * interface SensorReading { value: number; unit: string; }
   * const reading = msg.payloadAs<SensorReading>();
   * console.log(reading.value, reading.unit);
   * ```
   */
  payloadAs<T>(): T {
    return this.payload as T;
  }

  /**
   * Check if the payload has the exec-source shape (`{ stdout: string }`).
   *
   * Useful for detecting messages from the `exec` primitive before unwrapping.
   */
  hasStdoutPayload(): boolean {
    return typeof this.payload === "object" && this.payload !== null &&
      typeof (this.payload as Record<string, unknown>).stdout === "string";
  }

  /**
   * Unwrap an exec-source payload, extracting and parsing the `stdout` field.
   *
   * If the payload has `{ stdout: string }`, the stdout value is JSON-parsed
   * and a new `EmergentMessage` is returned with the parsed result as its
   * payload. If JSON parsing fails, the raw stdout string becomes the payload.
   *
   * If the payload does not have the exec-source shape, `this` is returned
   * unchanged.
   */
  unwrapStdout(): EmergentMessage {
    if (!this.hasStdoutPayload()) {
      return this;
    }

    const stdout = (this.payload as Record<string, unknown>).stdout as string;

    let parsed: unknown;
    try {
      parsed = JSON.parse(stdout);
    } catch {
      parsed = stdout;
    }

    return new EmergentMessage({
      id: this.id,
      messageType: this.messageType,
      source: this.source,
      correlationId: this.correlationId,
      causationId: this.causationId,
      timestampMs: this.timestampMs,
      payload: parsed,
      metadata: this.metadata,
    });
  }

  /**
   * Convert to JSON-serializable object.
   * @internal
   */
  toJSON(): Record<string, unknown> {
    return {
      id: this.id,
      message_type: this.messageType,
      source: this.source,
      correlation_id: this.correlationId,
      causation_id: this.causationId,
      timestamp_ms: this.timestampMs,
      payload: this.payload,
      metadata: this.metadata,
    };
  }

  /**
   * Create from wire format (snake_case).
   * @internal
   */
  static fromWire(wire: WireMessage): EmergentMessage {
    return new EmergentMessage({
      id: wire.id,
      messageType: wire.message_type,
      source: wire.source,
      correlationId: wire.correlation_id,
      causationId: wire.causation_id,
      timestampMs: wire.timestamp_ms,
      payload: wire.payload,
      metadata: wire.metadata as Record<string, unknown> | undefined,
    });
  }
}

/** Data for constructing an EmergentMessage */
export interface EmergentMessageData {
  id: string;
  messageType: string;
  source: string;
  correlationId?: string;
  causationId?: string;
  timestampMs: number;
  payload: unknown;
  metadata?: Record<string, unknown>;
}

/**
 * What the engine's IPC layer reports about itself.
 *
 * This describes the transport, not the workflow. It never lists Emergent
 * topics such as `timer.tick`, and it never lists sources, handlers or sinks.
 * Ask `EmergentSink.getTopology()` for the primitives and the topics they
 * publish and subscribe to.
 */
export interface DiscoveryInfo {
  /**
   * IPC type names the engine has registered, such as `EmergentMessage` and
   * `SystemEvent`. These are transport envelope names, not topics, and
   * subscribing to one delivers nothing useful.
   */
  readonly messageTypes: readonly string[];
  /**
   * Actors the engine exposes over IPC, such as `message_broker`. These are
   * engine internals, not the primitives in the config.
   */
  readonly primitives: readonly PrimitiveInfo[];
}

/**
 * An actor the engine exposes over IPC, as listed by `discover()`.
 */
export interface PrimitiveInfo {
  /** Name the actor is exposed under, such as `message_broker` */
  readonly name: string;
  /**
   * Never set by `discover()`: the engine's reply carries a name for each
   * actor and no kind.
   */
  readonly kind?: PrimitiveKind;
}

/**
 * The kind of primitive.
 */
export type PrimitiveKind = "Source" | "Handler" | "Sink";

/**
 * The lifecycle state of a primitive.
 */
export type PrimitiveState =
  | "configured"
  | "starting"
  | "running"
  | "stopping"
  | "stopped"
  | "failed"
  | "external";

/**
 * Detailed information about a primitive in the topology.
 */
export interface TopologyPrimitive {
  /** Unique name of the primitive. */
  readonly name: string;
  /** Kind of primitive (source, handler, sink). */
  readonly kind: string;
  /** Current lifecycle state. */
  readonly state: PrimitiveState;
  /** Message types this primitive publishes. */
  readonly publishes: readonly string[];
  /** Message types this primitive subscribes to. */
  readonly subscribes: readonly string[];
  /** Process ID if running. */
  readonly pid?: number;
  /** Error message if failed. */
  readonly error?: string;
}

/**
 * Current topology state (all primitives).
 */
export interface TopologyState {
  /** All primitives in the system. */
  readonly primitives: readonly TopologyPrimitive[];
}

/**
 * Options for connecting to the Emergent engine.
 */
export interface ConnectOptions {
  /** Custom socket path (overrides EMERGENT_SOCKET env var) */
  socketPath?: string;
  /** Connection timeout in milliseconds (default: 30000) */
  timeout?: number;
  /** Auto-reconnect on disconnect (default: false) */
  reconnect?: boolean;
}

// ============================================================================
// Internal Types (not exported from mod.ts)
// ============================================================================

/**
 * Wire format message (snake_case for JSON serialization).
 * @internal
 */
export interface WireMessage {
  id: string;
  message_type: string;
  source: string;
  correlation_id?: string;
  causation_id?: string;
  timestamp_ms: number;
  payload: unknown;
  metadata?: unknown;
}

/**
 * IPC push notification from acton-reactive.
 * @internal
 */
export interface IpcPushNotification {
  /** Transport-layer notification ID (NOT the message ID) */
  notification_id: string;
  /** The message type name */
  message_type: string;
  /** Source actor (optional) */
  source_actor?: string;
  /** The serialized EmergentMessage payload */
  payload: unknown;
  /** Timestamp when notification was created */
  timestamp_ms: number;
}

/**
 * IPC response from server.
 * @internal
 */
export interface IpcResponse {
  correlation_id: string;
  success: boolean;
  error?: string;
  error_code?: string;
  payload?: unknown;
}

/**
 * IPC subscribe request.
 * @internal
 */
export interface IpcSubscribeRequest {
  correlation_id: string;
  message_types: string[];
}

/**
 * IPC pattern subscribe request.
 *
 * Patterns are a prefix followed by one `*`, or `*` alone.
 * @internal
 */
export interface IpcPatternSubscribeRequest {
  correlation_id: string;
  patterns: string[];
}

/**
 * IPC subscription response.
 * @internal
 */
export interface IpcSubscriptionResponse {
  success: boolean;
  subscribed_types: string[];
  error?: string;
}

/**
 * IPC pattern subscription response.
 * @internal
 */
export interface IpcPatternSubscriptionResponse {
  correlation_id: string;
  success: boolean;
  subscribed_patterns: string[];
  error?: string;
}

/**
 * IPC discover request.
 * @internal
 */
export interface IpcDiscoverRequest {
  correlation_id: string;
  include_actors: boolean;
  include_message_types: boolean;
}

/**
 * IPC discover response.
 *
 * The engine writes these fields at the top level of the frame body, beside
 * `correlation_id` and `success`, and not under `payload`. It leaves out
 * whichever list was not asked for.
 * @internal
 */
export interface IpcDiscoverResponse {
  correlation_id: string;
  success: boolean;
  error?: string;
  protocol_version?: Record<string, unknown>;
  actors?: Array<{ name: string; ern?: string }>;
  message_types?: string[];
}

/**
 * IPC envelope for requests.
 * @internal
 */
export interface IpcEnvelope {
  correlation_id: string;
  target: string;
  message_type: string;
  payload: unknown;
  expects_reply: boolean;
}
