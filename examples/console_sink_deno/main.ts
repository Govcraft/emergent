#!/usr/bin/env -S deno run --allow-env --allow-read --allow-net=unix
/**
 * Console Sink Example - Prints timer ticks with colored output.
 *
 * The SDK automatically handles system.shutdown for graceful shutdown.
 * When the engine signals shutdown, the message stream closes gracefully.
 */

import { EmergentSink } from "../../sdks/ts/mod.ts";

const dim = "\x1b[2m";
const blue = "\x1b[34m";
const cyan = "\x1b[36m";
const reset = "\x1b[0m";

const name = Deno.env.get("EMERGENT_NAME") || "console_sink_deno";

// The SDK handles system.shutdown internally - stream closes gracefully on shutdown
for await (const msg of EmergentSink.messages(name, ["timer.tick"])) {
  const time = new Date(msg.timestampMs).toISOString().slice(11, 23);
  const { sequence } = msg.payloadAs<{ sequence: number }>();
  console.log(`${dim}[${time}]${reset} ${blue}${msg.messageType}${reset} ${cyan}#${sequence}${reset}`);
}
