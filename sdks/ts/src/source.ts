/**
 * EmergentSource - Publish-only client primitive.
 * @module
 */

import type {
  ConnectOptions,
  DiscoveryInfo,
  EmergentMessage,
} from "./types.ts";
import type { MessageBuilder } from "./message.ts";
import { BaseClient } from "./client.ts";
import { createMessage } from "./message.ts";

/**
 * A Source can only publish messages.
 *
 * Use this for ingress components that bring data into the Emergent system,
 * such as HTTP endpoints, sensors, or external API integrations.
 *
 * @example
 * ```typescript
 * // Simple usage with `using` for automatic cleanup
 * await using source = await EmergentSource.connect("my_source");
 *
 * // Shorthand publish: type + payload
 * await source.publish("sensor.reading", { value: 42.5, unit: "celsius" });
 *
 * // Full control with builder
 * await source.publish(
 *   createMessage("sensor.reading")
 *     .payload({ value: 42.5, unit: "celsius" })
 *     .metadata({ sensor_id: "temp-01" })
 * );
 * ```
 */
export class EmergentSource extends BaseClient
  implements Disposable, AsyncDisposable {
  private constructor(name: string, options?: ConnectOptions) {
    super(name, "Source", options);
  }

  /**
   * Connect to the Emergent engine as a Source.
   *
   * @param name - Unique name for this source
   * @param options - Connection options
   *
   * @example
   * ```typescript
   * const source = await EmergentSource.connect("my_source");
   * // ... use source ...
   * source.close();
   *
   * // Or with automatic cleanup:
   * await using source = await EmergentSource.connect("my_source");
   * ```
   */
  static async connect(
    name: string,
    options?: ConnectOptions,
  ): Promise<EmergentSource> {
    const source = new EmergentSource(name, options);
    await source.connectInternal(options?.socketPath);
    return source;
  }

  /**
   * Publish a message.
   *
   * Supports multiple calling patterns for maximum ergonomics:
   *
   * @example
   * ```typescript
   * // 1. Shorthand: type + payload (most common)
   * await source.publish("timer.tick", { count: 1 });
   *
   * // 2. MessageBuilder (auto-calls .build())
   * await source.publish(
   *   createMessage("timer.tick")
   *     .payload({ count: 1 })
   *     .metadata({ trace_id: "abc" })
   * );
   *
   * // 3. Complete EmergentMessage
   * await source.publish(message);
   * ```
   */
  async publish(
    messageOrType: EmergentMessage | MessageBuilder | string,
    payload?: unknown,
  ): Promise<void> {
    let message: EmergentMessage;

    if (typeof messageOrType === "string") {
      // Shorthand: publish("type", { payload })
      message = createMessage(messageOrType).payload(payload).build();
    } else if (
      "build" in messageOrType && typeof messageOrType.build === "function"
    ) {
      // MessageBuilder: auto-call build()
      message = messageOrType.build();
    } else {
      // Already an EmergentMessage
      message = messageOrType as EmergentMessage;
    }

    await this.publishInternal(message);
  }

  /**
   * Discover available message types and primitives.
   *
   * @example
   * ```typescript
   * const info = await source.discover();
   * console.log("Available types:", info.messageTypes);
   * console.log("Connected primitives:", info.primitives);
   * ```
   */
  async discover(): Promise<DiscoveryInfo> {
    return await this.discoverInternal();
  }

  /**
   * Implement `Symbol.dispose` for `using` declaration support.
   */
  [Symbol.dispose](): void {
    this.close();
  }

  /**
   * Implement `Symbol.asyncDispose` for `await using` declaration support.
   */
  async [Symbol.asyncDispose](): Promise<void> {
    await this.disconnect();
  }
}
