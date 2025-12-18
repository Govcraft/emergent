/**
 * EmergentHandler - Subscribe and publish client primitive.
 * @module
 */

import type {
  ConnectOptions,
  DiscoveryInfo,
  EmergentMessage,
} from "./types.ts";
import type { MessageBuilder } from "./message.ts";
import type { MessageStream } from "./stream.ts";
import { BaseClient } from "./client.ts";
import { createMessage } from "./message.ts";

/**
 * A Handler can both subscribe to and publish messages.
 *
 * Use this for processing components that transform, enrich, or route data.
 * Handlers are the workhorses of Emergent systems.
 *
 * @example
 * ```typescript
 * await using handler = await EmergentHandler.connect("order_processor");
 * await using stream = await handler.subscribe(["order.created"]);
 *
 * for await (const msg of stream) {
 *   const order = msg.payloadAs<Order>();
 *
 *   // Process and publish result with causation tracking
 *   await handler.publish(
 *     createMessage("order.processed")
 *       .causedBy(msg.id)
 *       .payload({ orderId: order.id, status: "ok" })
 *   );
 * }
 * ```
 */
export class EmergentHandler extends BaseClient
  implements Disposable, AsyncDisposable {
  private constructor(name: string, options?: ConnectOptions) {
    super(name, "Handler", options);
  }

  /**
   * Connect to the Emergent engine as a Handler.
   *
   * @param name - Unique name for this handler
   * @param options - Connection options
   *
   * @example
   * ```typescript
   * const handler = await EmergentHandler.connect("my_handler");
   * // ... use handler ...
   * handler.close();
   *
   * // Or with automatic cleanup:
   * await using handler = await EmergentHandler.connect("my_handler");
   * ```
   */
  static async connect(
    name: string,
    options?: ConnectOptions,
  ): Promise<EmergentHandler> {
    const handler = new EmergentHandler(name, options);
    await handler.connectInternal(options?.socketPath);
    return handler;
  }

  /**
   * Subscribe to message types and receive them via MessageStream.
   *
   * Supports both array and variadic arguments for convenience.
   *
   * @example
   * ```typescript
   * // Array style
   * const stream = await handler.subscribe(["order.created", "order.updated"]);
   *
   * // Variadic style
   * const stream = await handler.subscribe("order.created", "order.updated");
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
   * await handler.unsubscribe(["order.created"]);
   * ```
   */
  async unsubscribe(messageTypes: string[]): Promise<void> {
    await this.unsubscribeInternal(messageTypes);
  }

  /**
   * Publish a message.
   *
   * Supports multiple calling patterns for maximum ergonomics:
   *
   * @example
   * ```typescript
   * // 1. Shorthand: type + payload (most common)
   * await handler.publish("order.processed", { status: "ok" });
   *
   * // 2. With causation tracking (recommended for handlers)
   * await handler.publish(
   *   createMessage("order.processed")
   *     .causedBy(originalMsg.id)
   *     .payload({ status: "ok" })
   * );
   *
   * // 3. MessageBuilder (auto-calls .build())
   * await handler.publish(
   *   createMessage("order.processed")
   *     .causedBy(originalMsg.id)
   *     .payload({ status: "ok" })
   * );
   *
   * // 4. Complete EmergentMessage
   * await handler.publish(message);
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
   * const info = await handler.discover();
   * console.log("Available types:", info.messageTypes);
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
