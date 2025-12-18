#!/usr/bin/env -S deno run --allow-env --allow-read --allow-net=unix
/**
 * Console Sink Example - Prints timer ticks with colored output.
 */

import { EmergentSink } from "../../sdks/ts/mod.ts";

const dim = "\x1b[2m";
const blue = "\x1b[34m";
const cyan = "\x1b[36m";
const reset = "\x1b[0m";

const name = Deno.env.get("EMERGENT_NAME") || "console_sink_deno";

for await (const msg of EmergentSink.messages(name, ["timer.tick"])) {
  const time = new Date(msg.timestampMs).toISOString().replace("T", " ").replace("Z", "");
  const payload = msg.payloadAs<{ count: number }>();
  console.log(`${dim}[${time}]${reset} ${blue}${msg.messageType}${reset} ${cyan}#${payload.count}${reset}`);
}
