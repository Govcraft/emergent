/**
 * Tests for what the client does when the engine closes the connection.
 *
 * Nothing in flight can be answered once the connection is gone, so every
 * pending request fails at once with a ConnectionError, and the message stream
 * ends so a `for await` consumer stops.
 */

import { assertEquals, assertInstanceOf, assertRejects } from "@std/assert";
import { BaseClient } from "./client.ts";
import { ConnectionError } from "./errors.ts";
import { createMessage } from "./message.ts";
import { encodeFrame, MSG_TYPE_RESPONSE, tryDecodeFrame } from "./protocol.ts";
import type { MessageStream } from "./stream.ts";

/** Long enough that a request left to its timer fails the test on time alone. */
const TIMEOUT_MS = 60_000;
const AT_ONCE_MS = 2_000;

/** A conn whose one read stays open until the test ends the connection. */
class EndableConn {
  readonly frames: Uint8Array[] = [];
  #settle: {
    resolve: (n: number | null) => void;
    reject: (error: unknown) => void;
  } | null = null;

  write(bytes: Uint8Array): Promise<number> {
    this.frames.push(bytes);
    return Promise.resolve(bytes.length);
  }

  read(_: Uint8Array): Promise<number | null> {
    return new Promise((resolve, reject) => {
      this.#settle = { resolve, reject };
    });
  }

  /** The engine closed the connection: the read returns EOF. */
  eof(): void {
    this.#settle?.resolve(null);
  }

  /** The connection broke: the read itself fails. */
  reset(): void {
    this.#settle?.reject(new Deno.errors.ConnectionReset("reset by peer"));
  }

  /** What Deno does to a read in flight when the conn is closed under it. */
  close(): void {
    this.#settle?.reject(new Deno.errors.BadResource("Bad resource ID"));
  }
}

/** A client reading an `EndableConn`, with its protected calls opened up. */
class LossProbe extends BaseClient {
  readonly fake = new EndableConn();

  constructor() {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("loss-probe", "Handler", { timeout: TIMEOUT_MS });
    this.conn = this.fake as unknown as Deno.UnixConn;
    this.startReadLoop();
  }

  discover(): Promise<unknown> {
    return this.discoverInternal();
  }

  publishAck(): Promise<void> {
    return this.publishInternalAck(createMessage("ts80.event").build());
  }

  subscribe(): Promise<MessageStream> {
    return this.subscribeInternal(["ts80.event"]);
  }

  getTopology(): Promise<unknown> {
    return this.getTopologyInternal();
  }

  getMySubscriptions(): Promise<unknown> {
    return this.getMySubscriptionsInternal();
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

/** `running`, or a rejection when it is still unsettled after `AT_ONCE_MS`. */
async function atOnce<T>(running: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`still waiting after ${AT_ONCE_MS} ms`)),
      AT_ONCE_MS,
    );
  });
  try {
    return await Promise.race([running, late]);
  } finally {
    clearTimeout(timer);
  }
}

const ENDINGS: { name: string; end: (conn: EndableConn) => void }[] = [
  { name: "eof", end: (conn) => conn.eof() },
  { name: "read error", end: (conn) => conn.reset() },
];

const REQUESTS: {
  name: string;
  run: (probe: LossProbe) => Promise<unknown>;
}[] = [
  { name: "discover", run: (probe) => probe.discover() },
  { name: "publishAck", run: (probe) => probe.publishAck() },
  { name: "subscribe", run: (probe) => probe.subscribe() },
];

const QUERIES: {
  name: string;
  run: (probe: LossProbe) => Promise<unknown>;
}[] = [
  { name: "getTopology", run: (probe) => probe.getTopology() },
  { name: "getMySubscriptions", run: (probe) => probe.getMySubscriptions() },
];

// No close() before the assertions: it would fail the requests itself. Deno's
// leak check fails a step that leaves a request timer armed.
Deno.test("connection lost - a request in flight fails at once", async (t) => {
  for (const { name: ending, end } of ENDINGS) {
    for (const { name, run } of REQUESTS) {
      await t.step(`${name}, ${ending}`, async () => {
        const probe = new LossProbe();
        const running = run(probe);
        await until(() => probe.fake.frames.length === 1);

        end(probe.fake);
        const err = await assertRejects(() => atOnce(running));

        assertInstanceOf(err, ConnectionError);
        assertEquals(err.message, "Connection closed");
        probe.close();
      });
    }
  }
});

Deno.test("connection lost - a query in flight fails at once", async (t) => {
  for (const { name: ending, end } of ENDINGS) {
    for (const { name, run } of QUERIES) {
      await t.step(`${name}, ${ending}`, async () => {
        const probe = new LossProbe();
        const running = run(probe);
        await until(() => probe.fake.frames.length === 1);
        probe.acceptTheLastRequest();
        // The second frame is the query's request: it is now pending.
        await until(() => probe.fake.frames.length === 2);

        end(probe.fake);
        const err = await assertRejects(() => atOnce(running));

        assertInstanceOf(err, ConnectionError);
        assertEquals(err.message, "Connection closed");
        probe.close();
      });
    }
  }
});

Deno.test("connection lost - a for await consumer stops", async (t) => {
  for (const { name: ending, end } of ENDINGS) {
    await t.step(ending, async () => {
      const probe = new LossProbe();
      const subscribing = probe.subscribe();
      await until(() => probe.fake.frames.length === 1);
      probe.acceptTheLastRequest();
      const stream = await subscribing;

      const consuming = (async () => {
        let count = 0;
        for await (const _ of stream) count++;
        return count;
      })();
      end(probe.fake);

      assertEquals(await atOnce(consuming), 0);
      probe.close();
    });
  }
});

Deno.test("close - fails what is in flight the same way", async () => {
  const probe = new LossProbe();
  const running = probe.discover();
  await until(() => probe.fake.frames.length === 1);

  probe.close();
  const err = await assertRejects(() => atOnce(running));

  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Connection closed");
});
