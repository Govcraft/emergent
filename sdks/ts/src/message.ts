/**
 * Message creation and builder utilities.
 * @module
 */

import { EmergentMessage, type EmergentMessageData } from "./types.ts";
import { ValidationError } from "./errors.ts";

/**
 * Generate a unique message ID in MTI format.
 *
 * Format: `{prefix}_{timestamp_hex}{random_hex}`
 * This produces time-ordered, unique IDs similar to UUIDv7.
 */
export function generateMessageId(prefix = "msg"): string {
  const timestamp = Date.now().toString(16).padStart(12, "0");
  const random = Array.from(crypto.getRandomValues(new Uint8Array(8)))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `${prefix}_${timestamp}${random}`;
}

/**
 * Fluent builder for creating EmergentMessage instances.
 *
 * @example
 * ```typescript
 * // Simple message
 * const msg = createMessage("timer.tick").payload({ count: 1 }).build();
 *
 * // With causation tracking
 * const reply = createMessage("order.confirmed")
 *   .causedBy(originalMsg.id)
 *   .payload({ confirmed: true })
 *   .build();
 *
 * // With all options
 * const msg = createMessage("sensor.reading")
 *   .payload({ value: 42.5, unit: "celsius" })
 *   .metadata({ sensor_id: "temp-01", location: "room-a" })
 *   .source("sensor_service")
 *   .build();
 * ```
 */
export class MessageBuilder {
  #data: EmergentMessageData;

  /**
   * Create a new message builder.
   *
   * @param messageType - The message type (e.g., "timer.tick")
   */
  constructor(messageType: string) {
    if (!messageType || messageType.trim() === "") {
      throw new ValidationError("Message type cannot be empty", "messageType");
    }

    this.#data = {
      id: generateMessageId(),
      messageType,
      source: "",
      timestampMs: Date.now(),
      payload: null,
    };
  }

  /**
   * Set the message payload.
   *
   * @param data - The payload data (will be JSON serialized)
   */
  payload<T>(data: T): this {
    this.#data.payload = data;
    return this;
  }

  /**
   * Set the message metadata.
   *
   * @param meta - Metadata for tracing/debugging
   */
  metadata(meta: Record<string, unknown>): this {
    this.#data.metadata = meta;
    return this;
  }

  /**
   * Set the causation ID (for message chain tracking).
   *
   * Use this when creating a message in response to another message.
   *
   * @param messageId - The ID of the message that caused this one
   */
  causedBy(messageId: string): this {
    this.#data.causationId = messageId;
    return this;
  }

  /**
   * Set the correlation ID (for request-response patterns).
   *
   * @param correlationId - The correlation ID to link related messages
   */
  correlatedWith(correlationId: string): this {
    this.#data.correlationId = correlationId;
    return this;
  }

  /**
   * Set the source name.
   *
   * Note: This is typically set automatically by the client when publishing.
   *
   * @param name - The source name
   */
  source(name: string): this {
    this.#data.source = name;
    return this;
  }

  /**
   * Set a custom message ID.
   *
   * Note: In most cases you should not use this. IDs are auto-generated.
   *
   * @param id - The message ID
   */
  withId(id: string): this {
    this.#data.id = id;
    return this;
  }

  /**
   * Set a custom timestamp.
   *
   * Note: In most cases you should not use this. Timestamps are auto-generated.
   *
   * @param timestampMs - Unix timestamp in milliseconds
   */
  withTimestamp(timestampMs: number): this {
    this.#data.timestampMs = timestampMs;
    return this;
  }

  /**
   * Build the message.
   *
   * @returns A frozen, immutable EmergentMessage
   */
  build(): EmergentMessage {
    return new EmergentMessage(this.#data);
  }
}

/**
 * Create a new message builder.
 *
 * This is the primary way to create new messages for publishing.
 *
 * @param messageType - The message type (e.g., "timer.tick")
 *
 * @example
 * ```typescript
 * // Simple message
 * const msg = createMessage("timer.tick").payload({ count: 1 }).build();
 *
 * // Shorthand with source.publish()
 * await source.publish(createMessage("timer.tick").payload({ count: 1 }));
 * ```
 */
export function createMessage(messageType: string): MessageBuilder {
  return new MessageBuilder(messageType);
}
