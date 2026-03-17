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
   * Publish a message with broker acknowledgment (backpressure).
   *
   * Unlike {@link publish}, this waits for the engine's message broker to
   * confirm it has processed and forwarded the message before returning.
   *
   * Used internally by {@link publishAll} and {@link publishStream}.
   */
  async publishAck(
    messageOrType: EmergentMessage | MessageBuilder | string,
    payload?: unknown,
  ): Promise<void> {
    let message: EmergentMessage;

    if (typeof messageOrType === "string") {
      message = createMessage(messageOrType).payload(payload).build();
    } else if (
      "build" in messageOrType && typeof messageOrType.build === "function"
    ) {
      message = messageOrType.build();
    } else {
      message = messageOrType as EmergentMessage;
    }

    await this.publishInternalAck(message);
  }

  /**
   * Publish all messages from an iterable.
   *
   * Sends each message individually with broker acknowledgment so
   * subscribers begin consuming immediately. Stops on the first error.
   *
   * @returns The number of messages successfully published.
   */
  async publishAll(
    messages: Iterable<EmergentMessage | MessageBuilder>,
  ): Promise<number> {
    let count = 0;
    for (const msg of messages) {
      await this.publishAck(msg);
      count++;
    }
    return count;
  }

  /**
   * Publish messages from an async iterable (stream).
   *
   * Consumes the async iterable, publishing each message individually with
   * broker acknowledgment so subscribers begin consuming immediately.
   * Stops on the first publish error or when the iterable ends.
   *
   * @returns The number of messages successfully published.
   */
  async publishStream(
    messages: AsyncIterable<EmergentMessage | MessageBuilder>,
  ): Promise<number> {
    let count = 0;
    for await (const msg of messages) {
      await this.publishAck(msg);
      count++;
    }
    return count;
  }

  /**
   * Offer items as a pull-based stream with consumer-driven backpressure.
   *
   * Publishes `stream.ready`, then serves items one at a time as the
   * consumer sends `stream.pull` requests. Publishes `stream.end`
   * when exhausted.
   *
   * @returns The number of items published
   */
  async streamOffer(
    messageType: string,
    items: Iterable<unknown> | AsyncIterable<unknown>,
    pullStream: MessageStream,
    timeout = 30000,
  ): Promise<number> {
    const streamId = `strm_${Date.now()}_${Math.random().toString(36).slice(2, 10)}`;

    // Announce stream
    await this.publish(
      createMessage("stream.ready").payload({
        stream_id: streamId,
        message_type: messageType,
      }),
    );

    // Create iterator
    const isAsync = Symbol.asyncIterator in Object(items);
    const iter = isAsync
      ? (items as AsyncIterable<unknown>)[Symbol.asyncIterator]()
      : (items as Iterable<unknown>)[Symbol.iterator]();

    let published = 0;

    while (true) {
      // Wait for pull request
      const msg = await Promise.race([
        pullStream.next(),
        new Promise<null>((_, reject) =>
          setTimeout(() => reject(new Error("stream_offer timed out waiting for pull")), timeout)
        ),
      ]);

      if (msg === null) break;

      const isPull =
        msg.messageType === "stream.pull" &&
        typeof msg.payload === "object" &&
        msg.payload !== null &&
        (msg.payload as Record<string, unknown>).stream_id === streamId;

      if (isPull) {
        const next = isAsync
          ? await (iter as AsyncIterator<unknown>).next()
          : (iter as Iterator<unknown>).next();

        if (!next.done) {
          await this.publish(
            createMessage(messageType)
              .payload(next.value)
              .metadata({ stream_id: streamId }),
          );
          published++;
        } else {
          await this.publish(
            createMessage("stream.end").payload({ stream_id: streamId }),
          );
          break;
        }
      }
    }

    return published;
  }

  /**
   * Consume a pull-based stream, yielding items one at a time.
   *
   * Waits for `stream.ready`, then sends `stream.pull` requests
   * automatically after each item is consumed. Iteration stops on
   * `stream.end`.
   */
  async *streamConsume(
    messageType: string,
    sourceStream: MessageStream,
    timeout = 30000,
  ): AsyncIterable<EmergentMessage> {
    // Wait for stream.ready
    let streamId: string | null = null;
    while (streamId === null) {
      const msg = await Promise.race([
        sourceStream.next(),
        new Promise<null>((_, reject) =>
          setTimeout(() => reject(new Error("stream_consume timed out waiting for stream.ready")), timeout)
        ),
      ]);

      if (msg === null) {
        throw new Error("Source stream closed before stream.ready");
      }

      if (
        msg.messageType === "stream.ready" &&
        typeof msg.payload === "object" &&
        msg.payload !== null &&
        (msg.payload as Record<string, unknown>).message_type === messageType
      ) {
        streamId = (msg.payload as Record<string, unknown>).stream_id as string;
      }
    }

    // Send initial pull
    await this.publish(
      createMessage("stream.pull").payload({ stream_id: streamId }),
    );

    // Yield items
    while (true) {
      const msg = await Promise.race([
        sourceStream.next(),
        new Promise<null>((_, reject) =>
          setTimeout(() => reject(new Error("stream_consume timed out waiting for item")), timeout)
        ),
      ]);

      if (msg === null) break;

      // Check for stream.end
      if (
        msg.messageType === "stream.end" &&
        typeof msg.payload === "object" &&
        msg.payload !== null &&
        (msg.payload as Record<string, unknown>).stream_id === streamId
      ) {
        break;
      }

      // Check for matching data item
      if (
        msg.messageType === messageType &&
        typeof msg.metadata === "object" &&
        msg.metadata !== null &&
        (msg.metadata as Record<string, unknown>).stream_id === streamId
      ) {
        yield msg;
        // Request next
        await this.publish(
          createMessage("stream.pull").payload({ stream_id: streamId }),
        );
      }
    }
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
