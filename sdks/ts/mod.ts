/**
 * @emergent/client - TypeScript SDK for the Emergent workflow engine.
 *
 * This SDK provides type-safe primitives for building Emergent clients:
 * - {@link EmergentSource}: Publish messages (Sources can only publish)
 * - {@link EmergentHandler}: Subscribe and publish (Handlers do both)
 * - {@link EmergentSink}: Subscribe only (Sinks consume messages)
 *
 * @example Simple Sink (3 lines to consume messages)
 * ```typescript
 * import { EmergentSink } from "@emergent/client";
 *
 * for await (const msg of EmergentSink.messages("my_sink", ["timer.tick"])) {
 *   console.log(msg.payload);
 * }
 * ```
 *
 * @example Standard Sink (explicit lifecycle)
 * ```typescript
 * await using sink = await EmergentSink.connect("my_sink");
 * await using stream = await sink.subscribe(["timer.tick"]);
 *
 * for await (const msg of stream) {
 *   const data = msg.payloadAs<{ count: number }>();
 *   console.log(`Tick ${data.count}`);
 * }
 * ```
 *
 * @example Source (publishing)
 * ```typescript
 * await using source = await EmergentSource.connect("my_source");
 *
 * // Shorthand
 * await source.publish("timer.tick", { count: 1 });
 *
 * // Full control
 * await source.publish(
 *   createMessage("timer.tick")
 *     .payload({ count: 1 })
 *     .metadata({ trace_id: "abc" })
 * );
 * ```
 *
 * @example Handler (subscribe + publish with causation)
 * ```typescript
 * await using handler = await EmergentHandler.connect("my_handler");
 * await using stream = await handler.subscribe(["raw.event"]);
 *
 * for await (const msg of stream) {
 *   await handler.publish(
 *     createMessage("processed.event")
 *       .causedBy(msg.id)
 *       .payload({ processed: true })
 *   );
 * }
 * ```
 *
 * @module
 */

// ============================================================================
// Types
// ============================================================================

export { EmergentMessage } from "./src/types.ts";
export type {
  ConnectOptions,
  DiscoveryInfo,
  EmergentMessageData,
  PrimitiveInfo,
  PrimitiveKind,
  PrimitiveState,
  TopologyPrimitive,
  TopologyState,
} from "./src/types.ts";

// System Event Types
export type {
  SystemEventPayload,
  SystemShutdownPayload,
} from "./src/system-events.ts";
export {
  isSystemEventPayload,
  isSystemShutdownPayload,
  isErrorEvent,
  isSourceEvent,
  isHandlerEvent,
  isSinkEvent,
} from "./src/system-events.ts";

// ============================================================================
// Client Primitives
// ============================================================================

export { EmergentSource } from "./src/source.ts";
export { EmergentHandler } from "./src/handler.ts";
export { EmergentSink } from "./src/sink.ts";

// ============================================================================
// Message Building
// ============================================================================

export {
  createMessage,
  generateMessageId,
  MessageBuilder,
} from "./src/message.ts";

// ============================================================================
// Stream
// ============================================================================

export { MessageStream } from "./src/stream.ts";

// ============================================================================
// Errors
// ============================================================================

export {
  ConnectionError,
  DiscoveryError,
  DisposedError,
  EmergentError,
  ProtocolError,
  PublishError,
  SocketNotFoundError,
  SubscriptionError,
  TimeoutError,
  ValidationError,
} from "./src/errors.ts";

// ============================================================================
// Utilities
// ============================================================================

export { getSocketPath, socketExists } from "./src/client.ts";

// ============================================================================
// Helpers
// ============================================================================

export {
  HelperError,
  runHandler,
  runSink,
  runSource,
} from "./src/helpers.ts";
export type {
  HandlerProcessFn,
  SinkConsumeFn,
  SourceRunFn,
} from "./src/helpers.ts";
