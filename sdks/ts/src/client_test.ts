import { assertEquals } from "@std/assert";
import { BaseClient, extractShutdownKind, parseUnwrapFlag } from "./client.ts";
import { MessageStream } from "./stream.ts";
import type { PrimitiveKind } from "./types.ts";

// The same table runs in the Rust, Go and Python SDKs.
Deno.test("parseUnwrapFlag enables unwrapping for true and 1", () => {
  for (
    const value of ["true", "1", "TRUE", "True", " true ", " 1 ", "\ttrue\n"]
  ) {
    assertEquals(
      parseUnwrapFlag(value),
      true,
      `value: ${JSON.stringify(value)}`,
    );
  }
});

Deno.test("parseUnwrapFlag leaves unwrapping off for everything else", () => {
  const off = [
    undefined,
    "",
    " ",
    "false",
    "0",
    "no",
    "off",
    "yes",
    "on",
    "2",
    "11",
    "truee",
  ];
  for (const value of off) {
    assertEquals(
      parseUnwrapFlag(value),
      false,
      `value: ${JSON.stringify(value)}`,
    );
  }
});

/**
 * Build a `system.shutdown` payload exactly as the engine sends it.
 *
 * The engine forwards the whole serialized message as the notification
 * payload, so the targeted kind sits one level in.
 */
function engineShutdownEnvelope(kind: string): Record<string, unknown> {
  return {
    id: "msg_01m2xskqtyffgve4n1yw9vr8kz",
    message_type: "system.shutdown",
    source: "emergent",
    timestamp_ms: 1758318000000,
    payload: { kind },
  };
}

Deno.test("extractShutdownKind reads the engine envelope", () => {
  assertEquals(extractShutdownKind(engineShutdownEnvelope("sink")), "sink");
});

Deno.test("extractShutdownKind reads a bare kind object", () => {
  assertEquals(extractShutdownKind({ kind: "handler" }), "handler");
});

Deno.test("extractShutdownKind lowercases the kind", () => {
  assertEquals(extractShutdownKind(engineShutdownEnvelope("SINK")), "sink");
});

Deno.test("extractShutdownKind prefers the inner envelope", () => {
  assertEquals(
    extractShutdownKind({ kind: "source", payload: { kind: "sink" } }),
    "sink",
  );
});

Deno.test("extractShutdownKind reports nothing when no kind is there", () => {
  for (
    const payload of [
      {},
      { payload: {} },
      { payload: { kind: 7 } },
      { kind: null },
      "sink",
      null,
      undefined,
    ]
  ) {
    assertEquals(
      extractShutdownKind(payload),
      undefined,
      `payload: ${JSON.stringify(payload)}`,
    );
  }
});

/** Reaches the shutdown branch of the client without a socket. */
class ShutdownProbe extends BaseClient {
  constructor(kind: PrimitiveKind) {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("shutdown-probe", kind);
  }

  apply(
    notificationPayload: unknown,
    stream: MessageStream | null,
  ): MessageStream | null {
    return this.applyShutdownNotification(notificationPayload, stream);
  }
}

Deno.test("a matching shutdown closes the message stream", () => {
  const probe = new ShutdownProbe("Sink");
  const stream = new MessageStream();

  const kept = probe.apply(engineShutdownEnvelope("sink"), stream);

  assertEquals(stream.closed, true);
  assertEquals(kept, null);
});

Deno.test("a shutdown for another kind leaves the stream open", () => {
  const probe = new ShutdownProbe("Sink");
  const stream = new MessageStream();

  let kept: MessageStream | null = stream;
  for (const kind of ["source", "handler"]) {
    kept = probe.apply(engineShutdownEnvelope(kind), kept);
  }

  assertEquals(stream.closed, false);
  assertEquals(kept, stream);
});
