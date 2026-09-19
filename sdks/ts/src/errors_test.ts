/**
 * Tests for the error classes and the failure paths that throw them.
 *
 * Through SDK 0.13.1 `SubscriptionError`, `PublishError` and `DiscoveryError`
 * were exported and never thrown: every one of these failures threw a plain
 * `ConnectionError`. They are `ConnectionError` subclasses, so a handler
 * written against the old behavior still catches them.
 * @module
 */

import { assertEquals, assertInstanceOf, assertRejects } from "@std/assert";
import { BaseClient } from "./client.ts";
import {
  ConnectionError,
  DiscoveryError,
  DisposedError,
  EmergentError,
  ProtocolError,
  PublishError,
  SocketNotFoundError,
  SubscriptionError,
  TimeoutError,
  ValidationError,
} from "./errors.ts";
import { createMessage } from "./message.ts";
import {
  encodeFrame,
  MSG_TYPE_ERROR,
  MSG_TYPE_RESPONSE,
  tryDecodeFrame,
} from "./protocol.ts";

// ============================================================================
// The class hierarchy
// ============================================================================

Deno.test("each error carries its own name and code", () => {
  const cases: Array<[EmergentError, string, string]> = [
    [new ConnectionError("x"), "ConnectionError", "CONNECTION_FAILED"],
    [new SubscriptionError("x"), "SubscriptionError", "SUBSCRIPTION_FAILED"],
    [new PublishError("x"), "PublishError", "PUBLISH_FAILED"],
    [new DiscoveryError("x"), "DiscoveryError", "DISCOVERY_FAILED"],
    [new SocketNotFoundError("/s"), "SocketNotFoundError", "SOCKET_NOT_FOUND"],
    [new TimeoutError("x", 5), "TimeoutError", "TIMEOUT"],
    [new ProtocolError("x"), "ProtocolError", "PROTOCOL_ERROR"],
    [new DisposedError("Sink"), "DisposedError", "DISPOSED"],
    [new ValidationError("x", "f"), "ValidationError", "VALIDATION_ERROR"],
  ];
  for (const [err, name, code] of cases) {
    assertEquals(err.name, name);
    assertEquals(err.code, code, name);
    assertInstanceOf(err, EmergentError, name);
    assertInstanceOf(err, Error, name);
  }
});

Deno.test("the engine-rejection errors are connection errors", () => {
  const rejections = [
    new SubscriptionError("x", ["a.b"]),
    new PublishError("x", "a.b"),
    new DiscoveryError("x"),
  ];
  for (const err of rejections) {
    assertInstanceOf(err, ConnectionError, err.name);
  }
});

Deno.test("the other errors are not connection errors", () => {
  const others = [
    new SocketNotFoundError("/s"),
    new TimeoutError(),
    new ProtocolError("x"),
    new DisposedError("Sink"),
    new ValidationError("x", "f"),
  ];
  for (const err of others) {
    assertEquals(err instanceof ConnectionError, false, err.name);
  }
});

Deno.test("the rejection errors keep what was rejected", () => {
  assertEquals(new SubscriptionError("x", ["a.b"]).messageTypes, ["a.b"]);
  assertEquals(new SubscriptionError("x").messageTypes, []);
  assertEquals(new PublishError("x", "a.b").messageType, "a.b");
  assertEquals(new PublishError("x").messageType, "");
});

// ============================================================================
// The failure paths, fed frames without a socket
// ============================================================================

/** Stands in for the socket: records what the client writes. */
class FakeConn {
  readonly frames: Uint8Array[] = [];

  write(bytes: Uint8Array): Promise<number> {
    this.frames.push(bytes);
    return Promise.resolve(bytes.length);
  }

  close(): void {}
}

/** A client wired to a `FakeConn`, with its protected calls opened up. */
class RejectionProbe extends BaseClient {
  readonly fake = new FakeConn();

  constructor() {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("rejection-probe", "Handler", { timeout: 1000 });
    this.conn = this.fake as unknown as Deno.UnixConn;
  }

  readonly calls = {
    subscribe: (types: string[]) => this.subscribeInternal(types),
    publishAck: () =>
      this.publishInternalAck(createMessage("ts68.event").build()),
    discover: () => this.discoverInternal(),
    getTopology: () => this.getTopologyInternal(),
    getMySubscriptions: () => this.getMySubscriptionsInternal(),
  };

  /** Answer the request the client wrote last, as the engine does. */
  answer(success: boolean, error?: string): void {
    const frame = tryDecodeFrame(this.fake.frames[this.fake.frames.length - 1]);
    if (frame === null) throw new Error("the client wrote no complete frame");
    const { correlation_id } = frame.payload as { correlation_id: string };
    // The engine picks the frame type from the success flag.
    this.receiveBytes(
      encodeFrame(success ? MSG_TYPE_RESPONSE : MSG_TYPE_ERROR, {
        correlation_id,
        success,
        error,
      }),
    );
  }
}

/** Let a pending call move on to its next request. */
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

Deno.test("a rejected subscription throws a SubscriptionError", async () => {
  const probe = new RejectionProbe();
  const pending = probe.calls.subscribe(["ts68.event", "ts68.other.*"]);
  probe.answer(false, "Subscription limit reached");

  const err = await assertRejects(() => pending);
  assertInstanceOf(err, SubscriptionError);
  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Subscription limit reached");
  assertEquals(err.code, "SUBSCRIPTION_FAILED");
  assertEquals(err.messageTypes, ["ts68.event"]);
});

Deno.test("a rejected pattern subscription names the patterns", async () => {
  const probe = new RejectionProbe();
  const pending = probe.calls.subscribe(["ts68.event", "ts68.other.*"]);
  probe.answer(true);
  await tick();
  probe.answer(false, "Pattern limit reached");

  const err = await assertRejects(() => pending);
  assertInstanceOf(err, SubscriptionError);
  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Pattern limit reached");
  assertEquals(err.messageTypes, ["ts68.other.*"]);
});

Deno.test("a rejected acknowledged publish throws a PublishError", async () => {
  const probe = new RejectionProbe();
  const pending = probe.calls.publishAck();
  probe.answer(false, "Actor not found: message_broker");

  const err = await assertRejects(() => pending);
  assertInstanceOf(err, PublishError);
  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Actor not found: message_broker");
  assertEquals(err.code, "PUBLISH_FAILED");
  assertEquals(err.messageType, "ts68.event");
});

Deno.test("a rejected discovery throws a DiscoveryError", async () => {
  const probe = new RejectionProbe();
  const pending = probe.calls.discover();
  probe.answer(false, "Parse error: bad discover request");

  const err = await assertRejects(() => pending);
  assertInstanceOf(err, DiscoveryError);
  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Parse error: bad discover request");
  assertEquals(err.code, "DISCOVERY_FAILED");
});

Deno.test("a query whose reply subscription is rejected says which", async () => {
  const cases: Array<[string, (probe: RejectionProbe) => Promise<unknown>]> = [
    ["system.response.topology", (probe) => probe.calls.getTopology()],
    [
      "system.response.subscriptions",
      (probe) => probe.calls.getMySubscriptions(),
    ],
  ];

  for (const [replyType, call] of cases) {
    const probe = new RejectionProbe();
    const pending = call(probe);
    probe.answer(false);

    const err = await assertRejects(() => pending);
    assertInstanceOf(err, SubscriptionError, replyType);
    assertInstanceOf(err, ConnectionError, replyType);
    assertEquals(err.messageTypes, [replyType]);
  }
});
