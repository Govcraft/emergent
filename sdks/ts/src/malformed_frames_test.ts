/**
 * Tests for frames whose body is malformed.
 *
 * One bad frame must never end the read loop or close the subscriber stream.
 * It is logged and skipped, and the frames behind it in the same chunk are
 * still handled.
 * @module
 */

import { assertEquals, assertInstanceOf, assertThrows } from "@std/assert";
import {
  BaseClient,
  frameTypeLabel,
  pushFromFrame,
  subscribesFromPayload,
  topologyFromPayload,
  wireMessageFromPush,
} from "./client.ts";
import { ProtocolError } from "./errors.ts";
import {
  decodePayload,
  encodeFrame,
  FORMAT_JSON,
  FORMAT_MSGPACK,
  HEADER_SIZE,
  MAX_FRAME_SIZE,
  MSG_TYPE_PUSH,
  MSG_TYPE_RESPONSE,
  nextFrame,
  tryDecodeFrame,
} from "./protocol.ts";
import type { MessageStream } from "./stream.ts";
import type { TopologyPrimitive, TopologyState } from "./types.ts";

const WIRE_MESSAGE = {
  id: "msg_01",
  message_type: "ts64.event",
  source: "upstream",
  timestamp_ms: 1700000000000,
  payload: { n: 1 },
};

const NOTIFICATION = {
  notification_id: "ntf_01",
  message_type: "ts64.event",
  timestamp_ms: 1700000000001,
  payload: WIRE_MESSAGE,
};

/** A frame with the given body bytes, whatever they hold. */
function rawFrame(
  msgType: number,
  format: number,
  body: Uint8Array,
): Uint8Array {
  const frame = new Uint8Array(HEADER_SIZE + body.length);
  new DataView(frame.buffer).setUint32(0, body.length, false);
  frame[4] = 0x02;
  frame[5] = msgType;
  frame[6] = format;
  frame.set(body, HEADER_SIZE);
  return frame;
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, part) => n + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/** The body of an encoded frame, cut short by `drop` bytes. */
function truncatedBody(frame: Uint8Array, drop: number): Uint8Array {
  return frame.subarray(HEADER_SIZE, frame.length - drop);
}

const TRUNCATED_MSGPACK = truncatedBody(
  encodeFrame(MSG_TYPE_PUSH, NOTIFICATION),
  5,
);
const TRUNCATED_JSON = truncatedBody(
  encodeFrame(MSG_TYPE_PUSH, NOTIFICATION, FORMAT_JSON),
  5,
);

// ============================================================================
// decodePayload and nextFrame
// ============================================================================

Deno.test("a body that does not decode throws a ProtocolError", () => {
  const cases: Array<[string, Uint8Array, number]> = [
    ["truncated MessagePack", TRUNCATED_MSGPACK, FORMAT_MSGPACK],
    ["MessagePack cut after one byte", TRUNCATED_MSGPACK.subarray(0, 1), 2],
    ["truncated JSON", TRUNCATED_JSON, FORMAT_JSON],
    ["JSON that is not JSON", new TextEncoder().encode("nope"), FORMAT_JSON],
    ["unknown format", new Uint8Array([0x7b, 0x7d]), 0x09],
  ];
  for (const [label, bytes, format] of cases) {
    const err = assertThrows(() => decodePayload(bytes, format), label);
    assertInstanceOf(err, ProtocolError, label);
  }
});

Deno.test("nextFrame waits for a whole frame", () => {
  const frame = encodeFrame(MSG_TYPE_PUSH, NOTIFICATION);
  for (const length of [0, 3, HEADER_SIZE, frame.length - 1]) {
    assertEquals(
      nextFrame(frame.subarray(0, length)),
      { kind: "incomplete" },
      `length: ${length}`,
    );
  }
});

Deno.test("nextFrame decodes a whole frame in either format", () => {
  for (const format of [FORMAT_JSON, FORMAT_MSGPACK]) {
    const frame = encodeFrame(MSG_TYPE_PUSH, NOTIFICATION, format);
    assertEquals(nextFrame(concat(frame, new Uint8Array([1, 2, 3]))), {
      kind: "frame",
      frame: {
        msgType: MSG_TYPE_PUSH,
        format,
        payload: NOTIFICATION,
        bytesConsumed: frame.length,
      },
    });
  }
});

Deno.test("nextFrame reports a bad body with the length to skip", () => {
  const cases: Array<[string, number, Uint8Array]> = [
    ["truncated MessagePack", FORMAT_MSGPACK, TRUNCATED_MSGPACK],
    ["truncated JSON", FORMAT_JSON, TRUNCATED_JSON],
    ["unknown format", 0x09, new Uint8Array([0x7b, 0x7d])],
  ];
  for (const [label, format, body] of cases) {
    const step = nextFrame(rawFrame(MSG_TYPE_PUSH, format, body));
    assertEquals(step.kind, "bad-body", label);
    if (step.kind !== "bad-body") continue;
    assertEquals(step.msgType, MSG_TYPE_PUSH, label);
    assertEquals(step.bytesConsumed, HEADER_SIZE + body.length, label);
  }
});

Deno.test("nextFrame reports a header it cannot trust", () => {
  const tooLarge = new Uint8Array(HEADER_SIZE);
  new DataView(tooLarge.buffer).setUint32(0, MAX_FRAME_SIZE + 1, false);
  tooLarge[4] = 0x02;

  const wrongVersion = encodeFrame(MSG_TYPE_PUSH, NOTIFICATION);
  wrongVersion[4] = 0x01;

  for (const bytes of [tooLarge, wrongVersion]) {
    assertEquals(nextFrame(bytes).kind, "bad-framing");
    assertThrows(() => tryDecodeFrame(bytes), ProtocolError);
  }
});

Deno.test("tryDecodeFrame throws a ProtocolError for a bad body", () => {
  assertThrows(
    () =>
      tryDecodeFrame(
        rawFrame(MSG_TYPE_PUSH, FORMAT_MSGPACK, TRUNCATED_MSGPACK),
      ),
    ProtocolError,
  );
});

// ============================================================================
// pushFromFrame and wireMessageFromPush
// ============================================================================

Deno.test("pushFromFrame reads a notification with a string type", () => {
  assertEquals(pushFromFrame(NOTIFICATION), NOTIFICATION);
  assertEquals(pushFromFrame({ message_type: "a.b" })?.message_type, "a.b");
});

Deno.test("pushFromFrame reads nothing from a body of the wrong shape", () => {
  const bodies: unknown[] = [
    null,
    undefined,
    "text",
    7,
    [],
    [NOTIFICATION],
    {},
    { payload: WIRE_MESSAGE },
    { message_type: 7, payload: WIRE_MESSAGE },
    { message_type: null },
    { message_type: ["ts64.event"] },
  ];
  for (const body of bodies) {
    assertEquals(pushFromFrame(body), undefined, JSON.stringify(body));
  }
});

Deno.test("wireMessageFromPush reads a whole message", () => {
  const full = {
    ...WIRE_MESSAGE,
    correlation_id: "cor_1",
    causation_id: "msg_00",
    metadata: { trace: "t" },
  };
  assertEquals(wireMessageFromPush(full), full);
  assertEquals(wireMessageFromPush(WIRE_MESSAGE), {
    ...WIRE_MESSAGE,
    correlation_id: undefined,
    causation_id: undefined,
    metadata: undefined,
  });
});

Deno.test("wireMessageFromPush reads a null optional field as absent", () => {
  const wire = wireMessageFromPush({
    ...WIRE_MESSAGE,
    correlation_id: null,
    causation_id: null,
    metadata: null,
  });
  assertEquals(wire?.correlation_id, undefined);
  assertEquals(wire?.causation_id, undefined);
  assertEquals(wire?.metadata, undefined);
});

Deno.test("wireMessageFromPush keeps any payload", () => {
  for (const payload of [null, 0, "", [1], { a: { b: 1 } }]) {
    assertEquals(
      wireMessageFromPush({ ...WIRE_MESSAGE, payload })?.payload,
      payload,
    );
  }
});

Deno.test("wireMessageFromPush reads nothing from the wrong shape", () => {
  const { id: _id, ...noId } = WIRE_MESSAGE;
  const { timestamp_ms: _ts, ...noTimestamp } = WIRE_MESSAGE;
  const bodies: unknown[] = [
    null,
    undefined,
    "text",
    [],
    {},
    noId,
    noTimestamp,
    { ...WIRE_MESSAGE, id: 1 },
    { ...WIRE_MESSAGE, message_type: 2 },
    { ...WIRE_MESSAGE, source: [] },
    { ...WIRE_MESSAGE, source: null },
    { ...WIRE_MESSAGE, timestamp_ms: "1700000000000" },
    { ...WIRE_MESSAGE, correlation_id: 9 },
    { ...WIRE_MESSAGE, causation_id: {} },
  ];
  for (const body of bodies) {
    assertEquals(wireMessageFromPush(body), undefined, JSON.stringify(body));
  }
});

// ============================================================================
// topologyFromPayload, subscribesFromPayload and frameTypeLabel
// ============================================================================

Deno.test("topologyFromPayload drops entries of the wrong shape", () => {
  const timer: TopologyPrimitive = {
    name: "timer",
    kind: "source",
    state: "running",
    publishes: ["timer.tick"],
    subscribes: [],
  };
  const cases: Array<[unknown, TopologyPrimitive[]]> = [
    [{ primitives: [timer] }, [timer]],
    [{ primitives: [null, 7, "timer", {}, { name: 3 }, timer] }, [timer]],
    [{ primitives: "timer" }, []],
    [{ primitives: null }, []],
    [{}, []],
    [null, []],
    [undefined, []],
    ["text", []],
  ];
  for (const [payload, primitives] of cases) {
    assertEquals(
      topologyFromPayload(payload),
      { primitives },
      JSON.stringify(payload),
    );
  }
});

Deno.test("subscribesFromPayload drops entries that are not strings", () => {
  const cases: Array<[unknown, string[]]> = [
    [{ subscribes: ["a.b", "c.*"] }, ["a.b", "c.*"]],
    [{ subscribes: ["a.b", 7, null, ["c.d"], "e.f"] }, ["a.b", "e.f"]],
    [{ subscribes: "a.b" }, []],
    [{ subscribes: null }, []],
    [{}, []],
    [null, []],
    [undefined, []],
    [7, []],
  ];
  for (const [payload, subscribes] of cases) {
    assertEquals(
      subscribesFromPayload(payload),
      subscribes,
      JSON.stringify(payload),
    );
  }
});

Deno.test("frameTypeLabel names a known byte and spells out an unknown one", () => {
  assertEquals(frameTypeLabel(MSG_TYPE_PUSH), "PUSH");
  assertEquals(frameTypeLabel(0x0c), "0x0c");
});

// ============================================================================
// The client, fed frames without a socket
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

/** A subscribed client wired to a `FakeConn`. */
class PushProbe extends BaseClient {
  readonly fake = new FakeConn();

  constructor() {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("push-probe", "Sink", { timeout: 1000 });
    this.conn = this.fake as unknown as Deno.UnixConn;
  }

  feed(chunk: Uint8Array): void {
    this.receiveBytes(chunk);
  }

  /** Answer the request the client wrote last with a success. */
  acknowledge(): void {
    const frame = tryDecodeFrame(this.fake.frames[this.fake.frames.length - 1]);
    if (frame === null) throw new Error("the client wrote no complete frame");
    const { correlation_id } = frame.payload as { correlation_id: string };
    this.feed(
      encodeFrame(MSG_TYPE_RESPONSE, { correlation_id, success: true }),
    );
  }

  async subscribed(types: string[]): Promise<MessageStream> {
    const pending = this.subscribeInternal(types);
    this.acknowledge();
    return await pending;
  }

  /** Start a topology query and return it with its correlation id. */
  async topologyQuery(): Promise<
    { result: Promise<TopologyState>; correlationId: string }
  > {
    const result = this.getTopologyInternal();
    this.acknowledge();
    // Let the query move past its subscribe and write its request.
    await new Promise((resolve) => setTimeout(resolve, 0));
    const frame = tryDecodeFrame(this.fake.frames[this.fake.frames.length - 1]);
    const envelope = frame?.payload as {
      payload: { inner: { correlation_id: string } };
    };
    return { result, correlationId: envelope.payload.inner.correlation_id };
  }
}

const MALFORMED_FRAMES: Array<[string, Uint8Array]> = [
  ["PUSH with a null body", encodeFrame(MSG_TYPE_PUSH, null)],
  ["PUSH with an array body", encodeFrame(MSG_TYPE_PUSH, [NOTIFICATION])],
  [
    "PUSH with a numeric message_type",
    encodeFrame(MSG_TYPE_PUSH, { ...NOTIFICATION, message_type: 7 }),
  ],
  [
    "PUSH with a null message",
    encodeFrame(MSG_TYPE_PUSH, { ...NOTIFICATION, payload: null }),
  ],
  [
    "PUSH with wrong-typed message fields",
    encodeFrame(MSG_TYPE_PUSH, {
      ...NOTIFICATION,
      payload: { id: 1, message_type: 2, source: [], timestamp_ms: "x" },
    }),
  ],
  [
    "topology response with a null message",
    encodeFrame(MSG_TYPE_PUSH, {
      ...NOTIFICATION,
      message_type: "system.response.topology",
      payload: null,
    }),
  ],
  [
    "subscriptions response with a null message",
    encodeFrame(MSG_TYPE_PUSH, {
      ...NOTIFICATION,
      message_type: "system.response.subscriptions",
      payload: null,
    }),
  ],
  [
    "PUSH with a truncated MessagePack body",
    rawFrame(MSG_TYPE_PUSH, FORMAT_MSGPACK, TRUNCATED_MSGPACK),
  ],
  [
    "RESPONSE with a truncated MessagePack body",
    rawFrame(MSG_TYPE_RESPONSE, FORMAT_MSGPACK, TRUNCATED_MSGPACK),
  ],
  [
    "PUSH with a truncated JSON body",
    rawFrame(MSG_TYPE_PUSH, FORMAT_JSON, TRUNCATED_JSON),
  ],
  [
    "PUSH in an unknown format",
    rawFrame(MSG_TYPE_PUSH, 0x09, new Uint8Array([0x7b, 0x7d])),
  ],
];

Deno.test("a good frame behind a malformed one is still delivered", async () => {
  const good = encodeFrame(MSG_TYPE_PUSH, NOTIFICATION);

  for (const [label, bad] of MALFORMED_FRAMES) {
    const probe = new PushProbe();
    const stream = await probe.subscribed(["ts64.event"]);

    // One chunk, so a frame that stopped the loop would strand the good one.
    probe.feed(concat(bad, good));

    assertEquals(stream.closed, false, label);
    assertEquals(stream.pending, 1, label);
    const message = stream.tryNext();
    assertEquals(message?.id, WIRE_MESSAGE.id, label);
    assertEquals(message?.payload, WIRE_MESSAGE.payload, label);
    probe.close();
  }
});

Deno.test("every malformed frame in one chunk is skipped", async () => {
  const probe = new PushProbe();
  const stream = await probe.subscribed(["ts64.event"]);
  const good = encodeFrame(MSG_TYPE_PUSH, NOTIFICATION);

  probe.feed(
    concat(...MALFORMED_FRAMES.flatMap(([, bad]) => [bad, good])),
  );

  assertEquals(stream.closed, false);
  assertEquals(stream.pending, MALFORMED_FRAMES.length);
  probe.close();
});

Deno.test("a malformed frame split across chunks is skipped", async () => {
  const probe = new PushProbe();
  const stream = await probe.subscribed(["ts64.event"]);
  const chunk = concat(
    rawFrame(MSG_TYPE_PUSH, FORMAT_MSGPACK, TRUNCATED_MSGPACK),
    encodeFrame(MSG_TYPE_PUSH, NOTIFICATION),
  );

  probe.feed(chunk.subarray(0, 20));
  assertEquals(stream.pending, 0);
  probe.feed(chunk.subarray(20));

  assertEquals(stream.tryNext()?.id, WIRE_MESSAGE.id);
  probe.close();
});

Deno.test("a topology query survives a malformed reply before the real one", async () => {
  const probe = new PushProbe();
  const { result, correlationId } = await probe.topologyQuery();
  const reply = (payload: unknown) =>
    encodeFrame(MSG_TYPE_PUSH, {
      ...NOTIFICATION,
      message_type: "system.response.topology",
      payload,
    });
  const timer: TopologyPrimitive = {
    name: "timer",
    kind: "source",
    state: "running",
    publishes: [],
    subscribes: [],
  };

  probe.feed(concat(
    reply(null),
    reply({ ...WIRE_MESSAGE, correlation_id: 9 }),
    reply({
      ...WIRE_MESSAGE,
      message_type: "system.response.topology",
      correlation_id: correlationId,
      payload: { primitives: [null, timer] },
    }),
  ));

  assertEquals(await result, { primitives: [timer] });
  probe.close();
});
