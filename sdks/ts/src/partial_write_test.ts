/**
 * Tests for writing frames to a socket that takes only part of a buffer.
 *
 * `Deno.Conn.write` resolves to the number of bytes it wrote, which may be
 * fewer than it was given. Half a frame on the wire costs the engine its
 * framing for the rest of the connection.
 */

import { assertEquals, assertRejects } from "@std/assert";
import { BaseClient, writeAll } from "./client.ts";
import { ConnectionError } from "./errors.ts";
import { createMessage } from "./message.ts";
import { encodeFrame, MSG_TYPE_RESPONSE, nextFrame } from "./protocol.ts";
import type { EmergentMessage } from "./types.ts";

/** A conn that takes at most `step` bytes per call, after a macrotask. */
class TrickleConn {
  readonly wire: number[] = [];
  calls = 0;

  constructor(readonly step: number) {}

  async write(bytes: Uint8Array): Promise<number> {
    this.calls++;
    // Yield to the event loop, as a socket that is not writable yet does, so
    // a second writer gets the chance to cut in.
    await new Promise((resolve) => setTimeout(resolve, 0));
    const taken = bytes.subarray(0, this.step);
    this.wire.push(...taken);
    return taken.length;
  }

  close(): void {}
}

/** Decode every frame on the wire, failing on any byte that is not one. */
function framesOn(wire: number[]): unknown[] {
  let buffer = new Uint8Array(wire);
  const payloads: unknown[] = [];
  while (buffer.length > 0) {
    const step = nextFrame(buffer);
    if (step.kind !== "frame") {
      throw new Error(`wire does not hold whole frames: ${step.kind}`);
    }
    payloads.push(step.frame.payload);
    buffer = buffer.subarray(step.frame.bytesConsumed);
  }
  return payloads;
}

/** A message whose frame is a few slices long for every conn below. */
function padded(id: number): EmergentMessage {
  return createMessage("ts69.event")
    .payload({ id, pad: "y".repeat(30 * id) })
    .build();
}

/** The message ID inside a publish envelope. */
function publishedId(payload: unknown): string {
  const envelope = payload as { payload: { inner: { id: string } } };
  return envelope.payload.inner.id;
}

Deno.test("writeAll - writes every byte", async (t) => {
  const bytes = new Uint8Array(Array.from({ length: 20 }, (_, i) => i));
  const cases: { name: string; step: number; calls: number }[] = [
    { name: "one byte per call", step: 1, calls: 20 },
    { name: "three bytes per call", step: 3, calls: 7 },
    { name: "exactly the buffer", step: 20, calls: 1 },
    { name: "more room than bytes", step: 64, calls: 1 },
  ];

  for (const { name, step, calls } of cases) {
    await t.step(name, async () => {
      const conn = new TrickleConn(step);
      await writeAll(conn, bytes);
      assertEquals(conn.wire, [...bytes]);
      assertEquals(conn.calls, calls);
    });
  }

  await t.step("an empty buffer needs no call", async () => {
    const conn = new TrickleConn(3);
    await writeAll(conn, new Uint8Array(0));
    assertEquals(conn.calls, 0);
  });

  await t.step("a writer that takes nothing is an error", async () => {
    const stuck = { write: (_: Uint8Array) => Promise.resolve(0) };
    await assertRejects(
      () => writeAll(stuck, bytes),
      ConnectionError,
      "accepted no bytes",
    );
  });
});

/** A client wired to a `TrickleConn`, with its protected calls opened up. */
class TrickleProbe extends BaseClient {
  readonly trickle: TrickleConn;

  constructor(step: number) {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("trickle-probe", "Handler", { timeout: 1000 });
    this.trickle = new TrickleConn(step);
    this.conn = this.trickle as unknown as Deno.UnixConn;
  }

  publish(message: EmergentMessage): Promise<void> {
    return this.publishInternal(message);
  }

  discover(): Promise<unknown> {
    return this.discoverInternal();
  }

  feed(chunk: Uint8Array): void {
    this.receiveBytes(chunk);
  }
}

Deno.test("publish - a partial write still puts the whole frame on the wire", async () => {
  const probe = new TrickleProbe(3);
  const message = padded(1);
  await probe.publish(message);

  const frames = framesOn(probe.trickle.wire);
  assertEquals(frames.map(publishedId), [message.id]);
  probe.close();
});

Deno.test("publish - concurrent publishes never interleave", async () => {
  const probe = new TrickleProbe(5);
  const messages = [1, 2, 3, 4].map(padded);

  // All four start before the first has written its second slice.
  await Promise.all(messages.map((message) => probe.publish(message)));

  const frames = framesOn(probe.trickle.wire);
  assertEquals(frames.map(publishedId), messages.map((m) => m.id));
  probe.close();
});

Deno.test("request - a partial write still sends the whole request, in order with a publish", async () => {
  const probe = new TrickleProbe(4);
  const discovered = probe.discover();
  const published = probe.publish(padded(7));
  await published;

  const frames = framesOn(probe.trickle.wire) as {
    correlation_id: string;
    message_type?: string;
  }[];
  assertEquals(frames.length, 2);
  // The discover request went first and is whole.
  assertEquals(frames[1].message_type, "EmergentMessage");

  probe.feed(
    encodeFrame(MSG_TYPE_RESPONSE, {
      correlation_id: frames[0].correlation_id,
      success: true,
      payload: { message_types: [], actors: [] },
    }),
  );
  await discovered;
  probe.close();
});

Deno.test("publish - a failed write rejects and leaves the queue usable", async () => {
  const probe = new TrickleProbe(6);
  let failNext = true;
  const real = probe.trickle.write.bind(probe.trickle);
  probe.trickle.write = (bytes: Uint8Array) => {
    if (failNext) {
      failNext = false;
      return Promise.reject(new Error("broken pipe"));
    }
    return real(bytes);
  };

  const second = padded(2);
  await assertRejects(() => probe.publish(padded(1)), Error, "broken pipe");
  await probe.publish(second);

  const frames = framesOn(probe.trickle.wire);
  assertEquals(frames.map(publishedId), [second.id]);
  probe.close();
});
