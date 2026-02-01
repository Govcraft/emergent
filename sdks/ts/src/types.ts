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
 * Discovery information about the engine.
 */
export interface DiscoveryInfo {
  /** Available message types that can be subscribed to */
  readonly messageTypes: readonly string[];
  /** List of connected primitives */
  readonly primitives: readonly PrimitiveInfo[];
}

/**
 * Information about a registered primitive.
 */
export interface PrimitiveInfo {
  /** Name of the primitive */
  readonly name: string;
  /** Type of primitive (Source, Handler, Sink) */
  readonly kind: PrimitiveKind;
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
 * IPC subscription response.
 * @internal
 */
export interface IpcSubscriptionResponse {
  success: boolean;
  subscribed_types: string[];
  error?: string;
}

/**
 * IPC discover response.
 * @internal
 */
export interface IpcDiscoverResponse {
  message_types: string[];
  primitives: Array<{ name: string; kind: string }>;
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
