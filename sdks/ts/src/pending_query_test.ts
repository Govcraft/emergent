/**
 * Tests for what a topology or subscriptions query leaves behind when the
 * publish of its request fails.
 *
 * Both queries subscribe to their response type, register a pending entry with
 * a timeout timer, then publish the request. A refused publish has to take the
 * entry and the timer with it. Left armed, the timer rejects a promise nobody
 * holds, and an unhandled rejection ends a Deno or Node process.
 */

import { assertEquals, assertInstanceOf, assertRejects } from "@std/assert";
import { BaseClient } from "./client.ts";
import { PublishError } from "./errors.ts";
import { encodeFrame, MSG_TYPE_RESPONSE, tryDecodeFrame } from "./protocol.ts";

/** A conn that takes the first frame, the subscribe, and refuses the rest. */
class OneFrameConn {
  readonly frames: Uint8Array[] = [];

  write(bytes: Uint8Array): Promise<number> {
    if (this.frames.length > 0) {
      return Promise.reject(new Deno.errors.BrokenPipe("Broken pipe"));
    }
    this.frames.push(bytes);
    return Promise.resolve(bytes.length);
  }

  close(): void {}
}

/** A client wired to a `OneFrameConn`, with its protected calls opened up. */
class QueryProbe extends BaseClient {
  readonly fake = new OneFrameConn();

  constructor(timeout: number) {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("query-probe", "Sink", { timeout });
    this.conn = this.fake as unknown as Deno.UnixConn;
  }

  getTopology(): Promise<unknown> {
    return this.getTopologyInternal();
  }

  getMySubscriptions(): Promise<unknown> {
    return this.getMySubscriptionsInternal();
  }

  /** Answer the subscribe the client wrote, the way the engine does. */
  acceptTheSubscribe(): void {
    const frame = tryDecodeFrame(this.fake.frames[0]);
    if (frame === null) throw new Error("the client wrote no complete frame");
    const { correlation_id } = frame.payload as { correlation_id: string };
    this.receiveBytes(
      encodeFrame(MSG_TYPE_RESPONSE, { correlation_id, success: true }),
    );
  }
}

const QUERIES: {
  name: string;
  run: (probe: QueryProbe) => Promise<unknown>;
}[] = [
  { name: "getTopology", run: (probe) => probe.getTopology() },
  { name: "getMySubscriptions", run: (probe) => probe.getMySubscriptions() },
];

/** Run a query whose subscribe is accepted and whose request is refused. */
async function refusedQuery(
  probe: QueryProbe,
  run: (probe: QueryProbe) => Promise<unknown>,
): Promise<unknown> {
  const running = run(probe);
  probe.acceptTheSubscribe();
  return await assertRejects(() => running);
}

// No close() here: it would clear the timer itself. Deno's leak check fails
// the step when a timer the query armed is still running at the end.
Deno.test("query - a refused request publish leaves no timer armed", async (t) => {
  for (const { name, run } of QUERIES) {
    await t.step(name, async () => {
      const err = await refusedQuery(new QueryProbe(60_000), run);

      assertInstanceOf(err, PublishError);
      assertInstanceOf(err.cause, Deno.errors.BrokenPipe);
    });
  }
});

Deno.test("query - a refused request publish rejects nothing later", async (t) => {
  for (const { name, run } of QUERIES) {
    await t.step(name, async () => {
      const unhandled: unknown[] = [];
      const record = (event: PromiseRejectionEvent) => {
        event.preventDefault();
        unhandled.push(event.reason);
      };
      globalThis.addEventListener("unhandledrejection", record);
      try {
        await refusedQuery(new QueryProbe(20), run);
        // Past the query's own timeout.
        await new Promise((resolve) => setTimeout(resolve, 80));
      } finally {
        globalThis.removeEventListener("unhandledrejection", record);
      }

      assertEquals(unhandled, []);
    });
  }
});
