/**
 * Tests for how the client settles RESPONSE and ERROR frames.
 *
 * The engine answers a failed request with an ERROR frame (0x03) whose body is
 * the same response object a RESPONSE frame carries. The bodies below were
 * captured from an engine on acton-reactive 9.3.0, except where one says
 * otherwise.
 * @module
 */

import { assertEquals, assertInstanceOf, assertRejects } from "@std/assert";
import {
  BaseClient,
  DEFAULT_ERROR_TEXT,
  errorFrameText,
  responseFromFrame,
  settlementFor,
} from "./client.ts";
import { ConnectionError, TimeoutError } from "./errors.ts";
import { createMessage } from "./message.ts";
import {
  decodePayload,
  encodeFrame,
  FORMAT_JSON,
  FORMAT_MSGPACK,
  frameTypeName,
  MSG_TYPE_DISCOVER,
  MSG_TYPE_ERROR,
  MSG_TYPE_HEARTBEAT,
  MSG_TYPE_PUSH,
  MSG_TYPE_REQUEST,
  MSG_TYPE_RESPONSE,
  MSG_TYPE_STREAM,
  MSG_TYPE_SUBSCRIBE,
  MSG_TYPE_SUBSCRIBE_PATTERNS,
  MSG_TYPE_UNSUBSCRIBE,
  MSG_TYPE_UNSUBSCRIBE_PATTERNS,
  tryDecodeFrame,
} from "./protocol.ts";
import type { MessageStream } from "./stream.ts";
import type { EmergentMessage } from "./types.ts";

const ACTOR_NOT_FOUND = {
  correlation_id: "req_x",
  success: false,
  error: "Actor not found: no_such_actor",
  error_code: "ACTOR_NOT_FOUND",
};

const UNPARSEABLE_REQUEST = {
  correlation_id: "unknown",
  success: false,
  error: "Parse error: Serialization error: missing field `correlation_id`",
};

/**
 * Not captured: built from `IpcResponse::connection_rejected` in
 * acton-reactive 9.3.0, which the engine writes before closing a connection
 * it has no room for.
 */
const CONNECTION_REJECTED = {
  correlation_id: "__acton_connection_rejected__",
  success: false,
  error:
    "Server refused the connection: connection limit reached (1024 concurrent connections)",
  error_code: "CONNECTION_LIMIT_REACHED",
  payload: { limit: 1024 },
};

const OTHER_FRAME_TYPES = [
  MSG_TYPE_REQUEST,
  MSG_TYPE_HEARTBEAT,
  MSG_TYPE_PUSH,
  MSG_TYPE_SUBSCRIBE,
  MSG_TYPE_UNSUBSCRIBE,
  MSG_TYPE_DISCOVER,
  MSG_TYPE_STREAM,
  MSG_TYPE_SUBSCRIBE_PATTERNS,
  MSG_TYPE_UNSUBSCRIBE_PATTERNS,
];

const UNMATCHABLE_BODIES: unknown[] = [
  null,
  undefined,
  "text",
  [],
  {},
  { success: false, error: "no id" },
  { correlation_id: 7, success: false },
];

// ============================================================================
// responseFromFrame
// ============================================================================

Deno.test("an ERROR frame reads as a failure with its text", () => {
  assertEquals(responseFromFrame(MSG_TYPE_ERROR, ACTOR_NOT_FOUND), {
    correlation_id: "req_x",
    success: false,
    error: "Actor not found: no_such_actor",
    error_code: "ACTOR_NOT_FOUND",
  });
});

Deno.test("an ERROR frame is a failure whatever its success field says", () => {
  const bodies = [
    { correlation_id: "req_x", success: true },
    { correlation_id: "req_x" },
    { correlation_id: "req_x", success: true, error: "" },
  ];
  for (const body of bodies) {
    const response = responseFromFrame(MSG_TYPE_ERROR, body);
    assertEquals(response?.success, false, JSON.stringify(body));
    assertEquals(response?.error, DEFAULT_ERROR_TEXT, JSON.stringify(body));
  }
});

Deno.test("a RESPONSE frame is read as sent", () => {
  const body = { correlation_id: "req_y", success: true, payload: { ok: 1 } };
  assertEquals(responseFromFrame(MSG_TYPE_RESPONSE, body), body);
});

Deno.test("fields beside the shared ones are kept", () => {
  const body = {
    correlation_id: "sub_1",
    success: true,
    subscribed_types: ["a.b"],
  };
  const response = responseFromFrame(MSG_TYPE_RESPONSE, body) as
    | Record<string, unknown>
    | undefined;
  assertEquals(response?.subscribed_types, ["a.b"]);
});

Deno.test("a body that cannot be matched reads as nothing", () => {
  for (const body of UNMATCHABLE_BODIES) {
    const label = JSON.stringify(body);
    assertEquals(responseFromFrame(MSG_TYPE_ERROR, body), undefined, label);
    assertEquals(responseFromFrame(MSG_TYPE_RESPONSE, body), undefined, label);
  }
});

Deno.test("a RESPONSE body with no success flag reads as nothing", () => {
  assertEquals(
    responseFromFrame(MSG_TYPE_RESPONSE, { correlation_id: "req_z" }),
    undefined,
  );
});

Deno.test("other frame types carry no response", () => {
  for (const msgType of [...OTHER_FRAME_TYPES, 0x0c]) {
    assertEquals(
      responseFromFrame(msgType, ACTOR_NOT_FOUND),
      undefined,
      `msgType: ${msgType}`,
    );
  }
});

// ============================================================================
// errorFrameText
// ============================================================================

Deno.test("errorFrameText gives the text and the code", () => {
  assertEquals(
    errorFrameText(ACTOR_NOT_FOUND),
    "Actor not found: no_such_actor (ACTOR_NOT_FOUND)",
  );
});

Deno.test("errorFrameText gives the text alone when there is no code", () => {
  assertEquals(errorFrameText(UNPARSEABLE_REQUEST), UNPARSEABLE_REQUEST.error);
});

Deno.test("errorFrameText falls back when there is no text", () => {
  for (const body of [null, undefined, [], {}, { error: "" }, { error: 3 }]) {
    assertEquals(
      errorFrameText(body),
      DEFAULT_ERROR_TEXT,
      JSON.stringify(body),
    );
  }
});

// ============================================================================
// settlementFor
// ============================================================================

const waitingOn = (...ids: string[]) => (id: string) => ids.includes(id);

Deno.test("an ERROR for a pending request settles it as a failure", () => {
  assertEquals(
    settlementFor(MSG_TYPE_ERROR, ACTOR_NOT_FOUND, waitingOn("req_x")),
    { kind: "settle", response: ACTOR_NOT_FOUND },
  );
});

Deno.test("a RESPONSE for a pending request settles it", () => {
  const body = { correlation_id: "req_y", success: true };
  assertEquals(
    settlementFor(MSG_TYPE_RESPONSE, body, waitingOn("req_y")),
    { kind: "settle", response: body },
  );
});

Deno.test("an ERROR that matches no pending request is reported", () => {
  const cases: Array<[unknown, string]> = [
    [UNPARSEABLE_REQUEST, UNPARSEABLE_REQUEST.error],
    [
      CONNECTION_REJECTED,
      `${CONNECTION_REJECTED.error} (CONNECTION_LIMIT_REACHED)`,
    ],
    [ACTOR_NOT_FOUND, "Actor not found: no_such_actor (ACTOR_NOT_FOUND)"],
    [null, DEFAULT_ERROR_TEXT],
    [{ error: "no id" }, "no id"],
  ];
  for (const [body, text] of cases) {
    assertEquals(
      settlementFor(MSG_TYPE_ERROR, body, waitingOn("req_other")),
      { kind: "unmatched-error", text },
      JSON.stringify(body),
    );
  }
});

Deno.test("a RESPONSE that matches no pending request is passed over", () => {
  assertEquals(
    settlementFor(
      MSG_TYPE_RESPONSE,
      { correlation_id: "req_late", success: true },
      waitingOn(),
    ),
    { kind: "unmatched-response", correlationId: "req_late" },
  );
});

Deno.test("a RESPONSE body that cannot be matched is malformed", () => {
  for (const body of UNMATCHABLE_BODIES) {
    assertEquals(
      settlementFor(MSG_TYPE_RESPONSE, body, waitingOn("req_x")),
      { kind: "malformed" },
      JSON.stringify(body),
    );
  }
});

// ============================================================================
// Frame decoding
// ============================================================================

Deno.test("an empty frame body decodes to null in either format", () => {
  assertEquals(decodePayload(new Uint8Array(0), FORMAT_JSON), null);
  assertEquals(decodePayload(new Uint8Array(0), FORMAT_MSGPACK), null);
});

Deno.test("a bare heartbeat header decodes as a frame", () => {
  for (const format of [FORMAT_JSON, FORMAT_MSGPACK]) {
    const heartbeat = new Uint8Array([
      0,
      0,
      0,
      0,
      2,
      MSG_TYPE_HEARTBEAT,
      format,
    ]);
    assertEquals(tryDecodeFrame(heartbeat), {
      msgType: MSG_TYPE_HEARTBEAT,
      format,
      payload: null,
      bytesConsumed: 7,
    });
  }
});

Deno.test("frameTypeName names known bytes only", () => {
  assertEquals(frameTypeName(MSG_TYPE_ERROR), "ERROR");
  assertEquals(frameTypeName(MSG_TYPE_HEARTBEAT), "HEARTBEAT");
  assertEquals(frameTypeName(MSG_TYPE_STREAM), "STREAM");
  assertEquals(frameTypeName(0x0c), undefined);
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

/** A client wired to a `FakeConn`, with its protected calls opened up. */
class FrameProbe extends BaseClient {
  readonly fake = new FakeConn();

  constructor(timeout = 1000) {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("frame-probe", "Handler", { timeout });
    this.conn = this.fake as unknown as Deno.UnixConn;
  }

  publishAck(message: EmergentMessage): Promise<void> {
    return this.publishInternalAck(message);
  }

  subscribe(types: string[]): Promise<MessageStream> {
    return this.subscribeInternal(types);
  }

  feed(...chunks: Uint8Array[]): void {
    for (const chunk of chunks) this.receiveBytes(chunk);
  }

  /** The body of the last frame the client wrote. */
  lastRequest(): { msgType: number; body: { correlation_id: string } } {
    const frame = tryDecodeFrame(this.fake.frames[this.fake.frames.length - 1]);
    if (frame === null) throw new Error("the client wrote no complete frame");
    return {
      msgType: frame.msgType,
      body: frame.payload as { correlation_id: string },
    };
  }
}

Deno.test("publishAck rejects with the engine's error text", async () => {
  const probe = new FrameProbe();
  const pending = probe.publishAck(
    createMessage("ts59.event").payload({ n: 1 }).build(),
  );
  const { msgType, body } = probe.lastRequest();
  assertEquals(msgType, MSG_TYPE_REQUEST);

  probe.feed(encodeFrame(MSG_TYPE_ERROR, {
    ...ACTOR_NOT_FOUND,
    correlation_id: body.correlation_id,
  }));

  const err = await assertRejects(() => pending);
  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Actor not found: no_such_actor");
});

Deno.test("subscribe rejects with the engine's error text", async () => {
  const probe = new FrameProbe();
  const pending = probe.subscribe(["ts59.event"]);
  const { msgType, body } = probe.lastRequest();
  assertEquals(msgType, MSG_TYPE_SUBSCRIBE);

  probe.feed(encodeFrame(MSG_TYPE_ERROR, {
    correlation_id: body.correlation_id,
    success: false,
    error: "Subscription limit reached",
    subscribed_types: [],
  }));

  const err = await assertRejects(() => pending);
  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Subscription limit reached");
});

Deno.test("an ERROR for another request leaves this one waiting", async () => {
  const probe = new FrameProbe(50);
  const pending = probe.publishAck(createMessage("ts59.event").build());

  probe.feed(
    encodeFrame(MSG_TYPE_ERROR, UNPARSEABLE_REQUEST),
    encodeFrame(MSG_TYPE_ERROR, CONNECTION_REJECTED),
  );

  // Still waiting, so the only way out is its own timeout.
  const err = await assertRejects(() => pending);
  assertInstanceOf(err, TimeoutError);
});

Deno.test("heartbeat, stream and unknown frames are passed over", async () => {
  const probe = new FrameProbe();
  const pending = probe.publishAck(createMessage("ts59.event").build());
  const { body } = probe.lastRequest();

  const heartbeat = new Uint8Array([0, 0, 0, 0, 2, MSG_TYPE_HEARTBEAT, 1]);
  const unknown = new Uint8Array([0, 0, 0, 2, 2, 0x0c, 1, 0x7b, 0x7d]);
  const stream = encodeFrame(MSG_TYPE_STREAM, {
    correlation_id: "str_1",
    sequence: 0,
    is_final: true,
  });
  const ok = encodeFrame(MSG_TYPE_RESPONSE, {
    correlation_id: body.correlation_id,
    success: true,
  });

  // One chunk, so a frame that stopped the loop would strand the response.
  const chunk = new Uint8Array(
    heartbeat.length + unknown.length + stream.length + ok.length,
  );
  let offset = 0;
  for (const part of [heartbeat, unknown, stream, ok]) {
    chunk.set(part, offset);
    offset += part.length;
  }
  probe.feed(chunk);

  await pending;
});

Deno.test("a malformed response body does not throw", async () => {
  const probe = new FrameProbe();
  const pending = probe.publishAck(createMessage("ts59.event").build());
  const { body } = probe.lastRequest();

  probe.feed(
    encodeFrame(MSG_TYPE_RESPONSE, null),
    encodeFrame(MSG_TYPE_RESPONSE, ["not", "a", "response"]),
    encodeFrame(MSG_TYPE_ERROR, null),
    encodeFrame(MSG_TYPE_RESPONSE, {
      correlation_id: body.correlation_id,
      success: true,
    }),
  );

  await pending;
});
