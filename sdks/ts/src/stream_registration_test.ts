/**
 * Tests for which message stream the client feeds and closes.
 *
 * The client keeps one registered stream. A subscribe that throws never hands
 * its stream to the caller, so the stream must not stay registered and open,
 * queueing messages nobody will read.
 */

import { assertEquals, assertInstanceOf, assertRejects } from "@std/assert";
import { BaseClient } from "./client.ts";
import { ConnectionError, TimeoutError } from "./errors.ts";
import { encodeFrame, MSG_TYPE_RESPONSE, tryDecodeFrame } from "./protocol.ts";
import type { MessageStream } from "./stream.ts";

/** A conn that takes `accepts` frames and refuses the rest. */
class CountingConn {
  readonly frames: Uint8Array[] = [];
  readonly #accepts: number;

  constructor(accepts: number) {
    this.#accepts = accepts;
  }

  write(bytes: Uint8Array): Promise<number> {
    if (this.frames.length >= this.#accepts) {
      return Promise.reject(new Deno.errors.BrokenPipe("Broken pipe"));
    }
    this.frames.push(bytes);
    return Promise.resolve(bytes.length);
  }

  close(): void {}
}

/** A client wired to a `CountingConn`, with its protected calls opened up. */
class StreamProbe extends BaseClient {
  readonly fake: CountingConn;

  constructor(timeout: number, accepts: number) {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("stream-probe", "Sink", { timeout });
    this.fake = new CountingConn(accepts);
    this.conn = this.fake as unknown as Deno.UnixConn;
  }

  subscribe(messageTypes: string[]): Promise<MessageStream> {
    return this.subscribeInternal(messageTypes);
  }

  get stream(): MessageStream | null {
    return this.registeredStream;
  }

  /** Answer the last request the client wrote, the way the engine does. */
  acceptTheLastRequest(): void {
    const frame = tryDecodeFrame(this.fake.frames[this.fake.frames.length - 1]);
    if (frame === null) throw new Error("the client wrote no complete frame");
    const { correlation_id } = frame.payload as { correlation_id: string };
    this.receiveBytes(
      encodeFrame(MSG_TYPE_RESPONSE, { correlation_id, success: true }),
    );
  }
}

/** Wait for `condition` a turn of the event loop at a time. */
async function until(condition: () => boolean): Promise<void> {
  while (!condition()) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

Deno.test("a subscribe that throws closes and unregisters its stream", async (t) => {
  await t.step("when the request times out", async () => {
    const probe = new StreamProbe(20, 1);
    const subscribing = probe.subscribe(["ts82.event"]);
    const stream = probe.stream;

    await assertRejects(() => subscribing, TimeoutError);

    assertEquals(stream?.closed, true);
    assertEquals(probe.stream, null);
    probe.close();
  });

  await t.step("when the write is refused", async () => {
    const probe = new StreamProbe(60_000, 0);
    const subscribing = probe.subscribe(["ts82.event"]);
    const stream = probe.stream;

    const error = await assertRejects(() => subscribing, ConnectionError);

    assertInstanceOf(error.cause, Deno.errors.BrokenPipe);
    assertEquals(stream?.closed, true);
    assertEquals(probe.stream, null);
    probe.close();
  });

  await t.step("when the pattern request times out", async () => {
    const probe = new StreamProbe(20, 2);
    const subscribing = probe.subscribe(["ts82.event", "ts82.*"]);
    const stream = probe.stream;
    probe.acceptTheLastRequest();
    await until(() => probe.fake.frames.length === 2);

    await assertRejects(() => subscribing, TimeoutError);

    assertEquals(stream?.closed, true);
    assertEquals(probe.stream, null);
    probe.close();
  });

  await t.step("when the pattern write is refused", async () => {
    const probe = new StreamProbe(60_000, 1);
    const subscribing = probe.subscribe(["ts82.event", "ts82.*"]);
    const stream = probe.stream;
    probe.acceptTheLastRequest();

    await assertRejects(() => subscribing, ConnectionError);

    assertEquals(stream?.closed, true);
    assertEquals(probe.stream, null);
    probe.close();
  });
});

Deno.test("a subscribe that succeeds keeps its stream registered", async () => {
  const probe = new StreamProbe(60_000, 1);
  const subscribing = probe.subscribe(["ts82.event"]);
  probe.acceptTheLastRequest();

  const stream = await subscribing;

  assertEquals(probe.stream, stream);
  assertEquals(stream.closed, false);
  probe.close();
});

/** Subscribe with the engine's acceptance fed straight to the client. */
async function subscribed(
  probe: StreamProbe,
  messageType: string,
): Promise<MessageStream> {
  const subscribing = probe.subscribe([messageType]);
  probe.acceptTheLastRequest();
  return await subscribing;
}

/** Count what a `for await` over `stream` sees before the stream ends. */
async function consume(stream: MessageStream): Promise<number> {
  let count = 0;
  for await (const _ of stream) count++;
  return count;
}

/** `running`, or "still waiting" when it has not settled after two seconds. */
async function soon<T>(running: Promise<T>): Promise<T | "still waiting"> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<"still waiting">((resolve) => {
    timer = setTimeout(() => resolve("still waiting"), 2_000);
  });
  try {
    return await Promise.race([running, late]);
  } finally {
    clearTimeout(timer);
  }
}

Deno.test("a second subscribe ends the stream it replaces", async (t) => {
  await t.step(
    "its consumer stops and the new stream is registered",
    async () => {
      const probe = new StreamProbe(60_000, 2);
      const first = await subscribed(probe, "ts86.first");
      const consuming = consume(first);

      const second = await subscribed(probe, "ts86.second");

      assertEquals(await soon(consuming), 0);
      assertEquals(first.closed, true);
      assertEquals(probe.stream, second);
      assertEquals(second.closed, false);
      probe.close();
    },
  );

  await t.step("even when the second subscribe then fails", async () => {
    const probe = new StreamProbe(20, 2);
    const first = await subscribed(probe, "ts86.first");

    await assertRejects(() => probe.subscribe(["ts86.second"]), TimeoutError);

    assertEquals(first.closed, true);
    assertEquals(probe.stream, null);
    probe.close();
  });
});
