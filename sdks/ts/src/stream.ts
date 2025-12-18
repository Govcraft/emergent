/**
 * MessageStream for consuming messages from subscriptions.
 * @module
 */

import type { EmergentMessage } from "./types.ts";

/**
 * Async iterator for receiving messages from subscriptions.
 *
 * Implements `AsyncIterable` for use with `for await...of` loops.
 * Also implements `Disposable` for automatic cleanup with `using`.
 *
 * @example
 * ```typescript
 * const stream = await sink.subscribe(["timer.tick"]);
 *
 * for await (const msg of stream) {
 *   console.log(msg.messageType, msg.payload);
 *   if (shouldStop()) break;
 * }
 *
 * // Or use with `using` for automatic cleanup
 * using stream = await sink.subscribe(["timer.tick"]);
 * // stream is automatically closed when scope exits
 * ```
 */
export class MessageStream
  implements AsyncIterable<EmergentMessage>, Disposable {
  #messageQueue: EmergentMessage[] = [];
  #waitingResolve: ((value: EmergentMessage | null) => void) | null = null;
  #closed = false;
  #onClose?: () => void;

  /** @internal */
  constructor(onClose?: () => void) {
    this.#onClose = onClose;
  }

  /**
   * Push a message to the stream.
   * @internal
   */
  push(message: EmergentMessage): void {
    if (this.#closed) return;

    if (this.#waitingResolve) {
      this.#waitingResolve(message);
      this.#waitingResolve = null;
    } else {
      this.#messageQueue.push(message);
    }
  }

  /**
   * Get the next message from the stream.
   * Blocks until a message is available or the stream is closed.
   *
   * @returns The next message, or `null` if the stream is closed.
   */
  next(): Promise<EmergentMessage | null> {
    if (this.#closed && this.#messageQueue.length === 0) {
      return Promise.resolve(null);
    }

    if (this.#messageQueue.length > 0) {
      return Promise.resolve(this.#messageQueue.shift()!);
    }

    return new Promise((resolve) => {
      this.#waitingResolve = resolve;
    });
  }

  /**
   * Try to get the next message without blocking.
   *
   * @returns The next message, or `null` if no message is available.
   */
  tryNext(): EmergentMessage | null {
    if (this.#closed && this.#messageQueue.length === 0) {
      return null;
    }

    if (this.#messageQueue.length > 0) {
      return this.#messageQueue.shift()!;
    }

    return null;
  }

  /**
   * Check if the stream is closed.
   */
  get closed(): boolean {
    return this.#closed;
  }

  /**
   * Get the number of messages waiting in the queue.
   */
  get pending(): number {
    return this.#messageQueue.length;
  }

  /**
   * Close the stream.
   *
   * Any pending `next()` calls will resolve with `null`.
   * Further messages pushed to the stream will be discarded.
   */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;

    if (this.#waitingResolve) {
      this.#waitingResolve(null);
      this.#waitingResolve = null;
    }

    this.#onClose?.();
  }

  /**
   * Mark the stream as closed due to an error.
   * @internal
   */
  closeWithError(): void {
    this.close();
  }

  /**
   * Implement `Symbol.dispose` for `using` declaration support.
   */
  [Symbol.dispose](): void {
    this.close();
  }

  /**
   * Implement `Symbol.asyncIterator` for `for await...of` support.
   */
  async *[Symbol.asyncIterator](): AsyncGenerator<
    EmergentMessage,
    void,
    unknown
  > {
    try {
      while (!this.#closed) {
        const message = await this.next();
        if (message === null) break;
        yield message;
      }
    } finally {
      this.close();
    }
  }
}
