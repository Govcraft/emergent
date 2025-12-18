/**
 * EmergentSink - Subscribe-only client primitive.
 * @module
 */

import type {
  ConnectOptions,
  DiscoveryInfo,
  EmergentMessage,
} from "./types.ts";
import type { MessageStream } from "./stream.ts";
import { BaseClient } from "./client.ts";

/**
 * A Sink can only subscribe to messages (no publishing).
 *
 * Use this for egress components that send data out of the Emergent system,
 * such as database writers, notification senders, or external API clients.
 *
 * @example
 * ```typescript
 * // Simplest: one-liner consumption (recommended for simple cases)
 * for await (const msg of EmergentSink.messages("my_sink", ["timer.tick"])) {
 *   console.log(msg.payload);
 * }
 *
 * // Standard: explicit lifecycle with automatic cleanup
 * await using sink = await EmergentSink.connect("my_sink");
 * await using stream = await sink.subscribe(["timer.tick", "timer.filtered"]);
 *
 * for await (const msg of stream) {
 *   const data = msg.payloadAs<{ count: number }>();
 *   console.log(`Tick ${data.count}`);
 * }
 * ```
 */
export class EmergentSink extends BaseClient
  implements Disposable, AsyncDisposable {
  private constructor(name: string, options?: ConnectOptions) {
    super(name, "Sink", options);
  }

  /**
   * Connect to the Emergent engine as a Sink.
   *
   * @param name - Unique name for this sink
   * @param options - Connection options
   *
   * @example
   * ```typescript
   * const sink = await EmergentSink.connect("my_sink");
   * // ... use sink ...
   * sink.close();
   *
   * // Or with automatic cleanup:
   * await using sink = await EmergentSink.connect("my_sink");
   * ```
   */
  static async connect(
    name: string,
    options?: ConnectOptions,
  ): Promise<EmergentSink> {
    const sink = new EmergentSink(name, options);
    await sink.connectInternal(options?.socketPath);
    return sink;
  }

  /**
   * Convenience method for one-liner message consumption.
   *
   * Connects, subscribes, and yields messages. Automatically cleans up
   * when the iteration completes or breaks.
   *
   * @param name - Unique name for this sink
   * @param types - Message types to subscribe to
   * @param options - Connection options
   *
   * @example
   * ```typescript
   * // Minimal boilerplate consumption
   * for await (const msg of EmergentSink.messages("console_sink", ["timer.tick"])) {
   *   console.log(`[${msg.messageType}]`, msg.payload);
   *   if (shouldStop()) break; // Breaking auto-cleans up
   * }
   *
   * // With options
   * for await (const msg of EmergentSink.messages("my_sink", ["event.*"], {
   *   timeout: 60000
   * })) {
   *   processMessage(msg);
   * }
   * ```
   */
  static async *messages(
    name: string,
    types: string[],
    options?: ConnectOptions,
  ): AsyncGenerator<EmergentMessage, void, unknown> {
    const sink = await EmergentSink.connect(name, options);

    try {
      const stream = await sink.subscribe(types);

      try {
        for await (const msg of stream) {
          yield msg;
        }
      } finally {
        stream.close();
      }
    } finally {
      sink.close();
    }
  }

  /**
   * Subscribe to message types and receive them via MessageStream.
   *
   * Supports both array and variadic arguments for convenience.
   *
   * @example
   * ```typescript
   * // Array style
   * const stream = await sink.subscribe(["timer.tick", "timer.filtered"]);
   *
   * // Variadic style
   * const stream = await sink.subscribe("timer.tick", "timer.filtered");
   *
   * for await (const msg of stream) {
   *   console.log(msg.messageType, msg.payload);
   * }
   * ```
   */
  async subscribe(
    typesOrFirst: string[] | string,
    ...rest: string[]
  ): Promise<MessageStream> {
    const types = Array.isArray(typesOrFirst)
      ? typesOrFirst
      : [typesOrFirst, ...rest];

    return await this.subscribeInternal(types);
  }

  /**
   * Unsubscribe from message types.
   *
   * @example
   * ```typescript
   * await sink.unsubscribe(["timer.tick"]);
   * ```
   */
  async unsubscribe(messageTypes: string[]): Promise<void> {
    await this.unsubscribeInternal(messageTypes);
  }

  /**
   * Discover available message types and primitives.
   *
   * @example
   * ```typescript
   * const info = await sink.discover();
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

  // Note: Sinks do NOT have a publish() method - they are consume-only
}
