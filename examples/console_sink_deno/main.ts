#!/usr/bin/env -S deno run --allow-env --allow-read --allow-write --allow-net=unix
/**
 * Console Sink Example
 *
 * A Sink that subscribes to timer events and prints them to the console
 * with colored output. Demonstrates the Sink pattern in Emergent using
 * Deno/TypeScript.
 *
 * Usage:
 *   # Make sure EMERGENT_SOCKET is set by the engine, then:
 *   deno run --allow-env --allow-read --allow-write --allow-net=unix main.ts
 *
 *   # Or run directly if made executable:
 *   ./main.ts
 *
 * Environment Variables:
 *   EMERGENT_SOCKET - Path to the engine's Unix socket (set by engine)
 *   EMERGENT_NAME   - Name of this sink (set by engine, or use default)
 */

import {
  EmergentSink,
  type EmergentMessage,
  ConnectionError,
} from "./emergent_client.ts";

// ANSI color codes for pretty output
const colors = {
  reset: "\x1b[0m",
  bold: "\x1b[1m",
  dim: "\x1b[2m",
  red: "\x1b[31m",
  green: "\x1b[32m",
  yellow: "\x1b[33m",
  blue: "\x1b[34m",
  magenta: "\x1b[35m",
  cyan: "\x1b[36m",
  white: "\x1b[37m",
};

// Message type to color mapping
const messageColors: Record<string, string> = {
  "timer.tick": colors.blue,
  "timer.filtered": colors.green,
  "system.error": colors.red,
  "system.warning": colors.yellow,
};

/**
 * Format a timestamp for display.
 */
function formatTimestamp(timestampMs: number): string {
  const date = new Date(timestampMs);
  return date.toISOString().replace("T", " ").replace("Z", "");
}

/**
 * Get color for a message type.
 */
function getColor(messageType: string): string {
  return messageColors[messageType] || colors.white;
}

/**
 * Format and print a message to the console.
 */
function printMessage(msg: EmergentMessage): void {
  const timestamp = formatTimestamp(msg.timestamp_ms);
  const color = getColor(msg.message_type);

  // Header line
  console.log(
    `${colors.dim}[${timestamp}]${colors.reset} ` +
      `${color}${colors.bold}${msg.message_type}${colors.reset} ` +
      `${colors.dim}from${colors.reset} ${colors.cyan}${msg.source}${colors.reset}`
  );

  // Message ID line
  console.log(`  ${colors.dim}id:${colors.reset} ${msg.id}`);

  // Causation chain (if present)
  if (msg.causation_id) {
    console.log(
      `  ${colors.dim}caused by:${colors.reset} ${msg.causation_id}`
    );
  }

  // Payload
  const payload = msg.payload as Record<string, unknown>;
  if (payload && typeof payload === "object") {
    console.log(`  ${colors.dim}payload:${colors.reset}`);
    for (const [key, value] of Object.entries(payload)) {
      console.log(`    ${colors.yellow}${key}:${colors.reset} ${value}`);
    }
  } else if (payload !== null && payload !== undefined) {
    console.log(`  ${colors.dim}payload:${colors.reset} ${payload}`);
  }

  console.log(); // Empty line between messages
}

/**
 * Main entry point.
 */
async function main(): Promise<void> {
  // Get sink name from environment (set by engine) or use default
  const name = Deno.env.get("EMERGENT_NAME") || "console_sink_deno";

  // Parse command-line arguments for subscription topics
  // Default to timer.tick only (colored output for tick events)
  const args = Deno.args;
  let topics = ["timer.tick"];

  if (args.length > 0) {
    // Allow comma-separated or space-separated topics
    topics = args.flatMap((arg) => arg.split(",")).filter((t) => t.length > 0);
  }

  // Connect to the Emergent engine (silently - no startup messages)
  let sink: EmergentSink;
  try {
    sink = await EmergentSink.connect(name);
  } catch (err) {
    if (err instanceof ConnectionError) {
      console.error(`${colors.red}Connection failed:${colors.reset} ${err.message}`);
      Deno.exit(1);
    }
    throw err;
  }

  // Subscribe to topics
  let stream;
  try {
    stream = await sink.subscribe(topics);
  } catch (err) {
    console.error(`${colors.red}Subscription failed:${colors.reset} ${err}`);
    sink.close();
    Deno.exit(1);
  }

  // Handle shutdown signals (silently)
  const abortController = new AbortController();
  Deno.addSignalListener("SIGINT", () => {
    abortController.abort();
  });
  Deno.addSignalListener("SIGTERM", () => {
    abortController.abort();
  });

  // Process incoming messages
  try {
    for await (const msg of stream) {
      if (abortController.signal.aborted) {
        break;
      }
      printMessage(msg);
    }
  } catch (err) {
    if (!abortController.signal.aborted) {
      console.error(`${colors.red}Stream error:${colors.reset} ${err}`);
    }
  }

  // Cleanup (silently)
  sink.close();
}

// Run main
main().catch((err) => {
  console.error(`${colors.red}Fatal error:${colors.reset}`, err);
  Deno.exit(1);
});
