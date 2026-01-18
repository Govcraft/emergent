/**
 * Convenience functions for building Emergent primitives.
 *
 * These helpers eliminate the boilerplate code for connecting, handling signals,
 * and running the event loop. Developers only need to provide their business logic
 * as an async function.
 *
 * @example Source with custom logic (interval-based timer)
 * ```typescript
 * import { runSource } from "@emergent/client/helpers";
 * import { createMessage } from "@emergent/client";
 *
 * await runSource("my_timer", async (source, shutdown) => {
 *   let count = 0;
 *   const intervalId = setInterval(async () => {
 *     if (shutdown.aborted) return;
 *     count++;
 *     await source.publish(createMessage("timer.tick").payload({ count }));
 *   }, 3000);
 *
 *   // Wait for shutdown signal
 *   await new Promise<void>((resolve) => {
 *     shutdown.addEventListener("abort", () => {
 *       clearInterval(intervalId);
 *       resolve();
 *     });
 *   });
 * });
 * ```
 *
 * @example Source as HTTP webhook
 * ```typescript
 * import { runSource } from "@emergent/client/helpers";
 *
 * await runSource("webhook", async (source, shutdown) => {
 *   const server = Deno.serve({ port: 8080, signal: shutdown }, async (req) => {
 *     await source.publish(createMessage("webhook.received").payload(await req.json()));
 *     return new Response("OK");
 *   });
 *   await server.finished;
 * });
 * ```
 *
 * @example Source as one-shot function
 * ```typescript
 * import { runSource } from "@emergent/client/helpers";
 *
 * await runSource("init", async (source, _shutdown) => {
 *   await source.publish(createMessage("system.init"));
 *   // Function completes, source disconnects automatically
 * });
 * ```
 *
 * @example Handler with message transformation
 * ```typescript
 * import { runHandler } from "@emergent/client/helpers";
 * import { createMessage } from "@emergent/client";
 *
 * await runHandler("my_handler", ["timer.tick"], async (msg, handler) => {
 *   await handler.publish(
 *     createMessage("timer.processed")
 *       .causedBy(msg.id)
 *       .payload({ processed: true })
 *   );
 * });
 * ```
 *
 * @example Sink with message consumption
 * ```typescript
 * import { runSink } from "@emergent/client/helpers";
 *
 * await runSink("my_sink", ["timer.processed"], async (msg) => {
 *   console.log("Received:", msg.payload);
 * });
 * ```
 *
 * @module
 */

import type { EmergentMessage } from "./types.ts";
import { EmergentSource } from "./source.ts";
import { EmergentHandler } from "./handler.ts";
import { EmergentSink } from "./sink.ts";

/** Default environment variable name for the primitive name. */
const EMERGENT_NAME_ENV = "EMERGENT_NAME";

/**
 * Error thrown by helper functions.
 */
export class HelperError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "HelperError";
  }
}

/**
 * Callback signature for Source run function.
 *
 * @param source - Source instance for publishing messages
 * @param shutdown - AbortSignal that fires when shutdown is requested
 */
export type SourceRunFn = (
  source: EmergentSource,
  shutdown: AbortSignal
) => Promise<void>;

/**
 * Callback signature for Handler message processor.
 *
 * @param msg - Incoming message
 * @param handler - Handler instance for publishing output messages
 */
export type HandlerProcessFn = (
  msg: EmergentMessage,
  handler: EmergentHandler
) => Promise<void>;

/**
 * Callback signature for Sink message consumer.
 *
 * @param msg - Incoming message
 */
export type SinkConsumeFn = (msg: EmergentMessage) => Promise<void>;

/**
 * Resolve the primitive name from the provided option or environment variable.
 */
function resolveName(name: string | undefined, defaultName: string): string {
  if (name !== undefined) {
    return name;
  }
  const envName = Deno.env.get(EMERGENT_NAME_ENV);
  return envName ?? defaultName;
}

/**
 * Run a Source with custom logic.
 *
 * This function handles all the boilerplate for running a Source:
 * - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
 * - Connects to the Emergent engine
 * - Sets up SIGTERM/SIGINT signal handling for graceful shutdown
 * - Calls your function with the connected source and an AbortSignal
 * - Gracefully disconnects after your function completes
 *
 * Your function receives:
 * - `source: EmergentSource` - The connected source for publishing messages
 * - `shutdown: AbortSignal` - An abort signal that fires when shutdown is requested
 *
 * @param name - Optional name for this source. Falls back to `EMERGENT_NAME` env var,
 *   then to "source".
 * @param runFn - Async function that implements your source logic.
 *
 * @throws {HelperError} If connection fails or user function throws.
 *
 * @example Interval-based timer
 * ```typescript
 * await runSource("my_timer", async (source, shutdown) => {
 *   let count = 0;
 *   const intervalId = setInterval(async () => {
 *     if (shutdown.aborted) return;
 *     count++;
 *     await source.publish(createMessage("timer.tick").payload({ count }));
 *   }, 3000);
 *
 *   await new Promise<void>((resolve) => {
 *     shutdown.addEventListener("abort", () => {
 *       clearInterval(intervalId);
 *       resolve();
 *     });
 *   });
 * });
 * ```
 *
 * @example One-shot source
 * ```typescript
 * await runSource("init", async (source, _shutdown) => {
 *   await source.publish(createMessage("system.init"));
 * });
 * ```
 */
export async function runSource(
  name: string | undefined,
  runFn: SourceRunFn
): Promise<void> {
  const resolvedName = resolveName(name, "source");

  let source: EmergentSource;
  try {
    source = await EmergentSource.connect(resolvedName);
  } catch (e) {
    throw new HelperError(
      `failed to connect to Emergent engine as '${resolvedName}': ${e}`
    );
  }

  // Create AbortController for shutdown signaling
  const abortController = new AbortController();

  // Set up signal handlers for graceful shutdown
  const signalHandler = () => {
    abortController.abort();
  };

  Deno.addSignalListener("SIGTERM", signalHandler);
  Deno.addSignalListener("SIGINT", signalHandler);

  try {
    await runFn(source, abortController.signal);
  } catch (e) {
    if (e instanceof HelperError) {
      throw e;
    }
    throw new HelperError(`user function error: ${e}`);
  } finally {
    // Clean up
    Deno.removeSignalListener("SIGTERM", signalHandler);
    Deno.removeSignalListener("SIGINT", signalHandler);
    await source.disconnect();
  }
}

/**
 * Run a Handler with message processing.
 *
 * This function handles all the boilerplate for running a Handler:
 * - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
 * - Connects to the Emergent engine
 * - Subscribes to the specified message types
 * - Sets up SIGTERM/SIGINT signal handling for graceful shutdown
 * - Runs the message loop, calling your function for each message
 * - Gracefully disconnects on shutdown
 *
 * @param name - Optional name for this handler. Falls back to `EMERGENT_NAME` env var,
 *   then to "handler".
 * @param subscriptions - Message types to subscribe to.
 * @param processFn - Async function called for each message with (msg, handler).
 *
 * @throws {HelperError} If connection, subscription fails, or user function throws.
 *
 * @example
 * ```typescript
 * import { runHandler } from "@emergent/client/helpers";
 * import { createMessage } from "@emergent/client";
 *
 * await runHandler("my_handler", ["timer.tick"], async (msg, handler) => {
 *   await handler.publish(
 *     createMessage("timer.processed")
 *       .causedBy(msg.id)
 *       .payload({ processed: true })
 *   );
 * });
 * ```
 */
export async function runHandler(
  name: string | undefined,
  subscriptions: string[],
  processFn: HandlerProcessFn
): Promise<void> {
  const resolvedName = resolveName(name, "handler");

  let handler: EmergentHandler;
  try {
    handler = await EmergentHandler.connect(resolvedName);
  } catch (e) {
    throw new HelperError(
      `failed to connect to Emergent engine as '${resolvedName}': ${e}`
    );
  }

  let stream;
  try {
    stream = await handler.subscribe(subscriptions);
  } catch (e) {
    handler.close();
    throw new HelperError(`failed to subscribe: ${e}`);
  }

  let running = true;

  // Set up signal handlers for graceful shutdown
  const signalHandler = () => {
    running = false;
    stream.close();
  };

  Deno.addSignalListener("SIGTERM", signalHandler);
  Deno.addSignalListener("SIGINT", signalHandler);

  try {
    for await (const msg of stream) {
      if (!running) {
        break;
      }
      try {
        await processFn(msg, handler);
      } catch (e) {
        throw new HelperError(`user function error: ${e}`);
      }
    }
  } finally {
    // Clean up
    Deno.removeSignalListener("SIGTERM", signalHandler);
    Deno.removeSignalListener("SIGINT", signalHandler);
    stream.close();
    await handler.disconnect();
  }
}

/**
 * Run a Sink with message consumption.
 *
 * This function handles all the boilerplate for running a Sink:
 * - Resolves the name from the provided option, `EMERGENT_NAME` env var, or default
 * - Connects to the Emergent engine
 * - Subscribes to the specified message types
 * - Sets up SIGTERM/SIGINT signal handling for graceful shutdown
 * - Runs the message loop, calling your function for each message
 * - Gracefully disconnects on shutdown
 *
 * @param name - Optional name for this sink. Falls back to `EMERGENT_NAME` env var,
 *   then to "sink".
 * @param subscriptions - Message types to subscribe to.
 * @param consumeFn - Async function called for each message with (msg).
 *
 * @throws {HelperError} If connection, subscription fails, or user function throws.
 *
 * @example
 * ```typescript
 * import { runSink } from "@emergent/client/helpers";
 *
 * await runSink("my_sink", ["timer.processed"], async (msg) => {
 *   console.log("Received:", msg.payload);
 * });
 * ```
 */
export async function runSink(
  name: string | undefined,
  subscriptions: string[],
  consumeFn: SinkConsumeFn
): Promise<void> {
  const resolvedName = resolveName(name, "sink");

  let sink: EmergentSink;
  try {
    sink = await EmergentSink.connect(resolvedName);
  } catch (e) {
    throw new HelperError(
      `failed to connect to Emergent engine as '${resolvedName}': ${e}`
    );
  }

  let stream;
  try {
    stream = await sink.subscribe(subscriptions);
  } catch (e) {
    sink.close();
    throw new HelperError(`failed to subscribe: ${e}`);
  }

  let running = true;

  // Set up signal handlers for graceful shutdown
  const signalHandler = () => {
    running = false;
    stream.close();
  };

  Deno.addSignalListener("SIGTERM", signalHandler);
  Deno.addSignalListener("SIGINT", signalHandler);

  try {
    for await (const msg of stream) {
      if (!running) {
        break;
      }
      try {
        await consumeFn(msg);
      } catch (e) {
        throw new HelperError(`user function error: ${e}`);
      }
    }
  } finally {
    // Clean up
    Deno.removeSignalListener("SIGTERM", signalHandler);
    Deno.removeSignalListener("SIGINT", signalHandler);
    stream.close();
    await sink.disconnect();
  }
}
