/**
 * Tests for reading the engine's discovery reply.
 *
 * The engine writes `actors` and `message_types` at the top level of the
 * response, not under `payload`. The body below was captured from an engine on
 * acton-reactive 9.3.0.
 * @module
 */

import {
  assertEquals,
  assertInstanceOf,
  assertRejects,
  assertThrows,
} from "@std/assert";
import { BaseClient, discoveryInfoFromResponse } from "./client.ts";
import { ConnectionError, ProtocolError } from "./errors.ts";
import {
  encodeFrame,
  MSG_TYPE_DISCOVER,
  MSG_TYPE_ERROR,
  MSG_TYPE_RESPONSE,
  tryDecodeFrame,
} from "./protocol.ts";
import type { DiscoveryInfo } from "./types.ts";

const ENGINE_DISCOVERY_RESPONSE = {
  correlation_id: "disc_new",
  success: true,
  protocol_version: {
    current: 2,
    min_supported: 1,
    max_supported: 2,
    description: "v2 (multi-format, streaming, push, discovery)",
    capabilities: {
      messagepack: true,
      streaming: true,
      push: true,
      discovery: true,
    },
  },
  actors: [
    {
      name: "message_broker",
      ern:
        "ern:acton:reactive:component:message_broker_01m2xw6x0jfhsrmbxrg90t657d",
    },
  ],
  message_types: ["SystemEvent", "EmergentMessage"],
};

// ============================================================================
// discoveryInfoFromResponse
// ============================================================================

Deno.test("discoveryInfoFromResponse reads the engine response", () => {
  assertEquals(discoveryInfoFromResponse(ENGINE_DISCOVERY_RESPONSE), {
    messageTypes: ["SystemEvent", "EmergentMessage"],
    primitives: [{ name: "message_broker" }],
  });
});

Deno.test("the engine reports no kind", () => {
  const info = discoveryInfoFromResponse(ENGINE_DISCOVERY_RESPONSE);
  assertEquals(info.primitives.map((p) => p.kind), [undefined]);
});

Deno.test("a list the engine left out reads as empty", () => {
  const { actors: _actors, ...withoutActors } = ENGINE_DISCOVERY_RESPONSE;
  assertEquals(discoveryInfoFromResponse(withoutActors), {
    messageTypes: ["SystemEvent", "EmergentMessage"],
    primitives: [],
  });

  const { message_types: _types, ...withoutTypes } = ENGINE_DISCOVERY_RESPONSE;
  assertEquals(discoveryInfoFromResponse(withoutTypes), {
    messageTypes: [],
    primitives: [{ name: "message_broker" }],
  });

  assertEquals(
    discoveryInfoFromResponse({
      ...ENGINE_DISCOVERY_RESPONSE,
      actors: null,
      message_types: null,
    }),
    { messageTypes: [], primitives: [] },
  );
});

Deno.test("a payload key is not where the lists live", () => {
  const info = discoveryInfoFromResponse({
    correlation_id: "disc_old",
    success: true,
    payload: {
      message_types: ["timer.tick"],
      primitives: [{ name: "timer", kind: "Source" }],
    },
  });
  assertEquals(info, { messageTypes: [], primitives: [] });
});

Deno.test("entries of the wrong shape are dropped", () => {
  const info = discoveryInfoFromResponse({
    ...ENGINE_DISCOVERY_RESPONSE,
    actors: [{ name: "message_broker" }, { ern: "no name" }, "text", null, {
      name: 7,
    }],
    message_types: ["SystemEvent", 3, null],
  });
  assertEquals(info, {
    messageTypes: ["SystemEvent"],
    primitives: [{ name: "message_broker" }],
  });
});

Deno.test("the result is frozen", () => {
  const info = discoveryInfoFromResponse(ENGINE_DISCOVERY_RESPONSE);
  assertEquals(Object.isFrozen(info.messageTypes), true);
  assertEquals(Object.isFrozen(info.primitives), true);
  assertEquals(Object.isFrozen(info.primitives[0]), true);
});

Deno.test("a response that is not a discovery response is a protocol error", () => {
  const bodies: unknown[] = [
    null,
    undefined,
    "text",
    [],
    { ...ENGINE_DISCOVERY_RESPONSE, actors: "message_broker" },
    { ...ENGINE_DISCOVERY_RESPONSE, message_types: { 0: "SystemEvent" } },
  ];
  for (const body of bodies) {
    assertThrows(
      () => discoveryInfoFromResponse(body),
      ProtocolError,
      "Malformed discovery response",
      JSON.stringify(body),
    );
  }
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
}

/** A client wired to a `FakeConn`, with discovery opened up. */
class DiscoverProbe extends BaseClient {
  readonly fake = new FakeConn();

  constructor() {
    // Keep the probe's logger off the file system.
    Deno.env.set("EMERGENT_LOG", "off");
    super("discover-probe", "Sink", { timeout: 1000 });
    this.conn = this.fake as unknown as Deno.UnixConn;
  }

  discover(): Promise<DiscoveryInfo> {
    return this.discoverInternal();
  }

  feed(chunk: Uint8Array): void {
    this.receiveBytes(chunk);
  }

  /** The one request the client wrote. */
  request(): { msgType: number; body: Record<string, unknown> } {
    assertEquals(this.fake.frames.length, 1);
    const frame = tryDecodeFrame(this.fake.frames[0]);
    if (frame === null) throw new Error("the client wrote no complete frame");
    return {
      msgType: frame.msgType,
      body: frame.payload as Record<string, unknown>,
    };
  }
}

Deno.test("discover sends a DISCOVER frame and reads the flat reply", async () => {
  const probe = new DiscoverProbe();
  const pending = probe.discover();

  const { msgType, body } = probe.request();
  assertEquals(msgType, MSG_TYPE_DISCOVER);
  assertEquals(probe.fake.frames[0][5], 0x08);
  assertEquals(String(body.correlation_id).startsWith("disc_"), true);
  assertEquals(body.include_actors, true);
  assertEquals(body.include_message_types, true);

  probe.feed(encodeFrame(MSG_TYPE_RESPONSE, {
    ...ENGINE_DISCOVERY_RESPONSE,
    correlation_id: body.correlation_id,
  }));

  assertEquals(await pending, {
    messageTypes: ["SystemEvent", "EmergentMessage"],
    primitives: [{ name: "message_broker" }],
  });
});

Deno.test("an ERROR frame fails discover with the engine's text", async () => {
  const probe = new DiscoverProbe();
  const pending = probe.discover();
  const { body } = probe.request();

  probe.feed(encodeFrame(MSG_TYPE_ERROR, {
    correlation_id: body.correlation_id,
    success: false,
    error: "Parse error: bad discover request",
  }));

  const err = await assertRejects(() => pending);
  assertInstanceOf(err, ConnectionError);
  assertEquals(err.message, "Parse error: bad discover request");
});

Deno.test("a malformed discovery reply rejects as a protocol error", async () => {
  const probe = new DiscoverProbe();
  const pending = probe.discover();
  const { body } = probe.request();

  probe.feed(encodeFrame(MSG_TYPE_RESPONSE, {
    correlation_id: body.correlation_id,
    success: true,
    actors: "message_broker",
  }));

  await assertRejects(() => pending, ProtocolError, "actors is not a list");
});
