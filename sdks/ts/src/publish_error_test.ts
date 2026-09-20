/**
 * Tests for the error a fire-and-forget publish throws when the write fails.
 *
 * The socket's own error, such as `Deno.errors.BrokenPipe`, is not an
 * `EmergentError`, so a handler written against the SDK's error classes never
 * saw it.
 */

import { assertEquals, assertInstanceOf, assertRejects } from "@std/assert";
import { BaseClient } from "./client.ts";
import { ConnectionError, EmergentError, PublishError } from "./errors.ts";
import { createMessage } from "./message.ts";

/** A conn whose every write fails with `failure`. */
class BrokenConn {
  constructor(readonly failure: unknown) {}

  write(_: Uint8Array): Promise<number> {
    return Promise.reject(this.failure);
  }

  close(): void {}
}

/** A client wired to a `BrokenConn`, with its protected calls opened up. */
class BrokenProbe extends BaseClient {
  constructor(failure: unknown) {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("broken-probe", "Source", { timeout: 1000 });
    this.conn = new BrokenConn(failure) as unknown as Deno.UnixConn;
  }

  publish(): Promise<void> {
    return this.publishInternal(createMessage("ts75.event").build());
  }

  discover(): Promise<unknown> {
    return this.discoverInternal();
  }
}

const FAILURES: { name: string; failure: unknown; text: string }[] = [
  {
    name: "broken pipe",
    failure: new Deno.errors.BrokenPipe("Broken pipe (os error 32)"),
    text: "Broken pipe (os error 32)",
  },
  {
    name: "connection reset",
    failure: new Deno.errors.ConnectionReset("reset by peer"),
    text: "reset by peer",
  },
  {
    name: "bad resource",
    failure: new Deno.errors.BadResource("Bad resource ID"),
    text: "Bad resource ID",
  },
  { name: "a thrown string", failure: "socket gone", text: "socket gone" },
];

Deno.test("publish - a failed write throws a PublishError that keeps the cause", async (t) => {
  for (const { name, failure, text } of FAILURES) {
    await t.step(name, async () => {
      const probe = new BrokenProbe(failure);
      const err = await assertRejects(() => probe.publish());

      assertInstanceOf(err, PublishError);
      assertInstanceOf(err, ConnectionError);
      assertInstanceOf(err, EmergentError);
      assertEquals(err.code, "PUBLISH_FAILED");
      assertEquals(err.messageType, "ts75.event");
      assertEquals(err.message, `Failed to publish: ${text}`);
      assertEquals(err.cause, failure);
      probe.close();
    });
  }
});

Deno.test("request - a failed write throws a ConnectionError that keeps the cause", async (t) => {
  for (const { name, failure, text } of FAILURES) {
    await t.step(name, async () => {
      const probe = new BrokenProbe(failure);
      const err = await assertRejects(() => probe.discover());

      assertInstanceOf(err, ConnectionError);
      assertEquals(err.code, "CONNECTION_FAILED");
      assertEquals(err.message, `Failed to send: ${text}`);
      assertEquals(err.cause, failure);
      probe.close();
    });
  }
});
