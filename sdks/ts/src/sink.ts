/**
 * EmergentSink - Subscribe-only client primitive.
 * @module
 */

import type {
  ConnectOptions,
  DiscoveryInfo,
  EmergentMessage,
  TopologyState,
} from "./types.ts";
import type { MessageStream } from "./stream.ts";
import { BaseClient } from "./client.ts";

/**
 * Report whether the engine must be asked for the configured subscribe list.
 *
 * {@link EmergentSink.messages} only falls back to the engine's configuration
 * when the caller requested no topics of their own.
 *
 * @param requested - Topics the caller asked for
 * @returns True when the configured list is needed
 */
export function needsConfiguredTopics(requested: string[]): boolean {
  return requested.length === 0;
}

/**
 * Resolve which topics to subscribe to.
 *
 * Explicitly requested topics always win. The configured list is the fallback
 * for callers that pass nothing, which keeps the engine's TOML the source of
 * truth for sinks that do not hard-code their own subscriptions.
 *
 * @param requested - Topics the caller asked for
 * @param configured - Topics the engine has configured for this primitive
 * @returns The topics to subscribe to
 */
export function resolveTopics(
  requested: string[],
  configured: string[],
): string[] {
  return requested.length > 0 ? requested : configured;
}

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
   * The `types` you pass win. Pass an empty array to defer to the engine
   * instead: the sink then queries its configured `subscribes` list from the
   * engine's TOML and subscribes to that. The engine is only consulted when
   * `types` is empty.
   *
   * @param name - Unique name for this sink
   * @param types - Message types to subscribe to; empty falls back to the
   *   engine's configured subscriptions
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
   *
   * // Defer to the engine's configured `subscribes` list
   * for await (const msg of EmergentSink.messages("my_sink", [])) {
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
      // Only ask the engine when the caller requested nothing.
      const configuredTypes = needsConfiguredTopics(types)
        ? await sink.getMySubscriptions()
        : [];

      const stream = await sink.subscribe(
        resolveTopics(types, configuredTypes),
      );

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
   * A client has one live stream, so pass every topic to one call. A second
   * call ends the earlier stream and returns a new one. The engine keeps the
   * earlier subscriptions, so the new stream receives the earlier topics too.
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
   * Ask the engine's IPC layer what it exposes.
   *
   * The reply lists the IPC type names the engine has registered
   * (`EmergentMessage`, `SystemEvent`) and the actors it exposes over IPC
   * (`message_broker`). It never lists Emergent topics such as `timer.tick`,
   * and it never lists sources, handlers or sinks. `getTopology()` answers that
   * question.
   *
   * @example
   * ```typescript
   * const info = await sink.discover();
   * console.log("IPC type names:", info.messageTypes);
   * console.log("IPC-exposed actors:", info.primitives);
   * ```
   */
  async discover(): Promise<DiscoveryInfo> {
    return await this.discoverInternal();
  }

  /**
   * Get the configured subscription types for this primitive.
   *
   * Queries the engine's config service to get the message types
   * this sink should subscribe to based on the engine configuration.
   *
   * @example
   * ```typescript
   * const sink = await EmergentSink.connect("my_sink");
   * const types = await sink.getMySubscriptions();
   * console.log("Configured to receive:", types);
   * ```
   */
  async getMySubscriptions(): Promise<string[]> {
    return await this.getMySubscriptionsInternal();
  }

  /**
   * Get the current topology (all primitives and their state).
   *
   * Queries the engine to get the current state of all registered
   * primitives, including their publish/subscribe configuration.
   *
   * @example
   * ```typescript
   * const sink = await EmergentSink.connect("my_sink");
   * const topology = await sink.getTopology();
   * for (const prim of topology.primitives) {
   *   console.log(`${prim.name} (${prim.kind}): ${prim.state}`);
   * }
   * ```
   */
  async getTopology(): Promise<TopologyState> {
    return await this.getTopologyInternal();
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
