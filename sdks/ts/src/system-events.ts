/**
 * Typed payloads for system lifecycle events.
 *
 * The engine emits system events with these message types:
 * - `system.started.<name>` - Primitive started successfully
 * - `system.stopped.<name>` - Primitive stopped gracefully
 * - `system.error.<name>` - Primitive encountered an error
 * - `system.shutdown` - Engine is shutting down
 *
 * @example
 * ```typescript
 * import type { SystemEventPayload } from "@emergent/client";
 *
 * if (msg.messageType.startsWith("system.started.")) {
 *   const payload = msg.payloadAs<SystemEventPayload>();
 *   console.log(`${payload.name} (${payload.kind}) started`);
 *   console.log(`Publishes: ${payload.publishes?.join(", ")}`);
 * }
 * ```
 *
 * @module
 */

/**
 * Payload for system lifecycle events (`system.started.*`, `system.stopped.*`, `system.error.*`).
 *
 * This interface represents the payload for primitive lifecycle events. The presence
 * of specific fields varies by event type:
 *
 * - `system.started.*`: Always has `pid`, never has `error`
 * - `system.stopped.*`: May have `pid`, never has `error`
 * - `system.error.*`: May have `pid`, always has `error`
 */
export interface SystemEventPayload {
  /** Name of the primitive (e.g., "timer", "my_handler"). */
  readonly name: string;

  /** Kind of the primitive ("source", "handler", or "sink"). */
  readonly kind: "source" | "handler" | "sink";

  /** Process ID if available. */
  readonly pid?: number;

  /** Message types this primitive publishes (Sources and Handlers). */
  readonly publishes?: readonly string[];

  /** Message types this primitive subscribes to (Handlers and Sinks). */
  readonly subscribes?: readonly string[];

  /** Error message if this is an error event. */
  readonly error?: string;
}

/**
 * Payload for `system.shutdown` events.
 *
 * This is sent by the engine when it is shutting down, signaling
 * all primitives to gracefully stop. The shutdown is phased:
 * 1. Sources receive shutdown first
 * 2. Handlers receive shutdown after sources exit
 * 3. Sinks receive shutdown after handlers exit
 */
export interface SystemShutdownPayload {
  /**
   * The type of primitive this shutdown is targeting.
   * One of "source", "handler", or "sink".
   */
  readonly kind: "source" | "handler" | "sink";
}

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Type guard to check if a payload is a system event payload.
 */
export function isSystemEventPayload(
  payload: unknown
): payload is SystemEventPayload {
  if (typeof payload !== "object" || payload === null) return false;
  const p = payload as Record<string, unknown>;
  return (
    typeof p.name === "string" &&
    typeof p.kind === "string" &&
    ["source", "handler", "sink"].includes(p.kind)
  );
}

/**
 * Type guard to check if a payload is a shutdown payload.
 */
export function isSystemShutdownPayload(
  payload: unknown
): payload is SystemShutdownPayload {
  if (typeof payload !== "object" || payload === null) return false;
  const p = payload as Record<string, unknown>;
  return (
    typeof p.kind === "string" &&
    ["source", "handler", "sink"].includes(p.kind) &&
    !("name" in p)
  );
}

/**
 * Check if a system event payload represents an error event.
 */
export function isErrorEvent(payload: SystemEventPayload): boolean {
  return payload.error !== undefined;
}

/**
 * Check if a system event payload represents a source primitive.
 */
export function isSourceEvent(payload: SystemEventPayload): boolean {
  return payload.kind === "source";
}

/**
 * Check if a system event payload represents a handler primitive.
 */
export function isHandlerEvent(payload: SystemEventPayload): boolean {
  return payload.kind === "handler";
}

/**
 * Check if a system event payload represents a sink primitive.
 */
export function isSinkEvent(payload: SystemEventPayload): boolean {
  return payload.kind === "sink";
}
