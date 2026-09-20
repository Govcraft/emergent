/**
 * Tests for what runSource does when the engine closes the connection.
 *
 * A Source subscribes to nothing, so no stream ends to tell it the engine is
 * gone. The abort signal is the only thing its function waits on, so a lost
 * connection has to abort it, the way SIGTERM does.
 */

import { assertEquals } from "@std/assert";
import { runSource } from "./helpers.ts";
import { createMessage } from "./message.ts";
import { EmergentSource } from "./source.ts";

const AT_ONCE_MS = 2_000;

/** A conn whose one read stays open until the test ends the connection. */
class EndableConn {
  #settle: {
    resolve: (n: number | null) => void;
    reject: (error: unknown) => void;
  } | null = null;

  write(bytes: Uint8Array): Promise<number> {
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

/**
 * Run `body` with `Deno.connect` handing out `conn`.
 *
 * The test gate has no write permission, so nothing here can listen on a real
 * Unix socket. The socket path only has to exist, and this file does.
 */
async function withEngineConn(
  conn: EndableConn,
  body: () => Promise<void>,
): Promise<void> {
  // Keep the client's logger off the file system.
  Deno.env.set("EMERGENT_LOG", "off");
  Deno.env.set("EMERGENT_SOCKET", new URL(import.meta.url).pathname);
  const connect = Deno.connect;
  Object.defineProperty(Deno, "connect", {
    configurable: true,
    value: () => Promise.resolve(conn),
  });
  try {
    await body();
  } finally {
    Object.defineProperty(Deno, "connect", {
      configurable: true,
      value: connect,
    });
    Deno.env.delete("EMERGENT_SOCKET");
  }
}

/** Resolves to "gave up" when `running` is still unsettled after `AT_ONCE_MS`. */
async function atOnce(
  running: Promise<void>,
  giveUp: () => void,
): Promise<"settled" | "gave up"> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<"gave up">((resolve) => {
    timer = setTimeout(() => resolve("gave up"), AT_ONCE_MS);
  });
  const outcome = await Promise.race([
    running.then((): "settled" => "settled"),
    late,
  ]);
  clearTimeout(timer);
  if (outcome === "gave up") {
    // Let the source function return, so the failing run leaves nothing open.
    giveUp();
    await running;
  }
  return outcome;
}

const ENDINGS: { name: string; end: (conn: EndableConn) => void }[] = [
  { name: "eof", end: (conn) => conn.eof() },
  { name: "read error", end: (conn) => conn.reset() },
];

Deno.test("a lost connection aborts the runSource signal", async (t) => {
  for (const ending of ENDINGS) {
    await t.step(ending.name, async () => {
      const conn = new EndableConn();
      await withEngineConn(conn, async () => {
        let giveUp = () => {};
        let published = () => {};
        const hasPublished = new Promise<void>((resolve) => {
          published = resolve;
        });

        const running = runSource("ts83", async (source, shutdown) => {
          await source.publish(createMessage("ts83.event"));
          published();
          await new Promise<void>((resolve) => {
            giveUp = resolve;
            shutdown.addEventListener("abort", () => resolve());
          });
        });
        await hasPublished;

        ending.end(conn);

        assertEquals(await atOnce(running, () => giveUp()), "settled");
      });
    });
  }
});

Deno.test("a source function that returns first sees no abort", async () => {
  const conn = new EndableConn();
  await withEngineConn(conn, async () => {
    const seen: boolean[] = [];

    await runSource("ts83", async (source, shutdown) => {
      await source.publish(createMessage("ts83.event"));
      seen.push(shutdown.aborted);
    });

    assertEquals(seen, [false]);
  });
});

Deno.test("close() is not a lost connection", async () => {
  const conn = new EndableConn();
  await withEngineConn(conn, async () => {
    const told: string[] = [];
    const source = await EmergentSource.connect("ts83");
    source.whenConnectionLost(() => told.push("lost"));

    source.close();
    await new Promise((resolve) => setTimeout(resolve, 20));

    assertEquals(told, []);
  });
});

Deno.test("a connection already lost is reported at once", async () => {
  const conn = new EndableConn();
  await withEngineConn(conn, async () => {
    const told: string[] = [];
    const source = await EmergentSource.connect("ts83");
    conn.eof();
    await new Promise((resolve) => setTimeout(resolve, 20));

    source.whenConnectionLost(() => told.push("lost"));

    assertEquals(told, ["lost"]);
    source.close();
  });
});
