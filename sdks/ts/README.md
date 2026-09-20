# @govcraft/emergent

TypeScript SDK for building event-driven workflows on the
[Emergent](https://github.com/govcraft/emergent) engine. Connect to a running
engine over Unix IPC and publish or consume messages through three typed
primitives: **Source**, **Handler**, and **Sink**.

```typescript
import { EmergentSink } from "@govcraft/emergent";

for await (const msg of EmergentSink.messages("my_sink", ["timer.tick"])) {
  console.log(msg.payload);
}
```

## Install

```bash
deno add jsr:@govcraft/emergent
```

Then import:

```typescript
import {
  EmergentHandler,
  EmergentSink,
  EmergentSource,
} from "@govcraft/emergent";
```

Or import directly without a local install:

```typescript
import { EmergentSink } from "jsr:@govcraft/emergent";
```

## Three Primitives

Every Emergent workflow is composed of Sources, Handlers, and Sinks. Each
primitive has a single, well-defined role:

| Primitive   | Subscribe | Publish | Role                                        |
| ----------- | --------- | ------- | ------------------------------------------- |
| **Source**  | --        | Yes     | Ingress -- bring data into the system       |
| **Handler** | Yes       | Yes     | Processing -- transform, enrich, or route   |
| **Sink**    | Yes       | --      | Egress -- persist, display, or forward data |

## Quick Start

### Sink -- consume messages

A Sink subscribes to message types and processes each one as it arrives.
`EmergentSink.messages` is a convenience method that connects, subscribes, and
yields messages in a single call:

```typescript
import { EmergentSink } from "jsr:@govcraft/emergent";

for await (const msg of EmergentSink.messages("my_sink", ["timer.tick"])) {
  const data = msg.payloadAs<{ sequence: number }>();
  console.log(`Tick #${data.sequence}`);
}
```

For explicit lifecycle control, connect and subscribe separately:

```typescript
await using sink = await EmergentSink.connect("my_sink");
const stream = await sink.subscribe(["timer.tick", "timer.filtered"]);

for await (const msg of stream) {
  console.log(msg.messageType, msg.payload);
}
```

### Source -- publish messages

A Source publishes messages into the engine. It cannot subscribe:

```typescript
import { EmergentSource } from "jsr:@govcraft/emergent";

await using source = await EmergentSource.connect("my_source");
await source.publish("sensor.reading", { value: 42.5, unit: "celsius" });
```

### Handler -- subscribe and publish

A Handler subscribes to incoming messages and publishes new ones. Use `causedBy`
to link output messages to the input that triggered them:

```typescript
import { createMessage, EmergentHandler } from "jsr:@govcraft/emergent";

await using handler = await EmergentHandler.connect("order_processor");
const stream = await handler.subscribe(["order.created"]);

for await (const msg of stream) {
  await handler.publish(
    createMessage("order.processed")
      .causedBy(msg.id)
      .payload({ status: "ok" }),
  );
}
```

## Publishing Messages

Every primitive that can publish supports three calling styles. All three
produce the same result:

```typescript
// Shorthand -- type string and payload object
await source.publish("timer.tick", { count: 1 });

// MessageBuilder -- fluent API with auto-build
await source.publish(
  createMessage("timer.tick").payload({ count: 1 }),
);

// Full EmergentMessage object
await source.publish(message);
```

## Streaming Publish

Publish a collection or async stream of messages. Each message is sent
individually so subscribers begin consuming immediately. Both methods return the
count of successfully published messages and stop on the first error.

```typescript
// From an array or any Iterable
const messages = records.map((r) =>
  createMessage("record.imported").payload(r)
);
const count = await source.publishAll(messages);

// From an async generator or any AsyncIterable
async function* generateMessages() {
  for (let i = 0; i < 100; i++) {
    yield createMessage("batch.item").payload({ index: i });
  }
}
const count = await source.publishStream(generateMessages());
```

Both `publishAll` and `publishStream` are available on `EmergentSource` and
`EmergentHandler`.

## Building Messages

`createMessage` returns a fluent builder for constructing immutable
`EmergentMessage` instances:

```typescript
import { createMessage } from "jsr:@govcraft/emergent";

const msg = createMessage("sensor.reading")
  .payload({ value: 42.5, unit: "celsius" })
  .metadata({ sensor_id: "temp-01", location: "room-a" })
  .build();
```

Link messages into traceable chains with `causedBy` and `correlatedWith`:

```typescript
const reply = createMessage("order.confirmed")
  .causedBy(originalMsg.id)
  .correlatedWith(requestId)
  .payload({ confirmed: true })
  .build();
```

The builder sets `id` (TypeID format) and `timestampMs` automatically. Call
`.build()` explicitly when you need the message object, or pass the builder
directly to `publish()`, which calls `.build()` for you.

## Subscribing to Messages

`subscribe` accepts an array or variadic arguments:

```typescript
// Array form
const stream = await sink.subscribe(["timer.tick", "timer.filtered"]);

// Variadic form
const stream = await sink.subscribe("timer.tick", "timer.filtered");
```

Iterate over the returned `MessageStream` with `for await...of`:

```typescript
for await (const msg of stream) {
  const data = msg.payloadAs<{ count: number }>();
  console.log(data.count);
}
```

`MessageStream` implements `AsyncIterable` and `Disposable`, so you can use
`using` for automatic cleanup:

```typescript
using stream = await sink.subscribe(["timer.tick"]);
```

The stream also ends when the connection to the engine is lost, so the loop
stops and the code after it runs. See [Error Handling](#error-handling).

A client has one live stream, so call `subscribe` once with every topic. After
SDK release 0.13.1 a second `subscribe` on the same client ends the earlier
stream, so a `for await` over it stops, and returns a new one. The engine keeps
the earlier subscriptions, so the new stream receives the earlier topics as well
as the new ones. The earlier stream ends when the second call starts, even if
that call then fails. On 0.13.1 and earlier the earlier stream was left open and
unfed, and nothing ever ended it, not even `close()`. To read two sets of topics
apart from each other, connect two clients.

## Resource Cleanup

All primitives implement `Disposable` and `AsyncDisposable`. Use `using` or
`await using` for automatic cleanup, or call `close()` manually:

```typescript
// Automatic cleanup (recommended)
await using sink = await EmergentSink.connect("my_sink");

// Manual cleanup
const handler = await EmergentHandler.connect("my_handler");
// ... use handler ...
handler.close();
```

The SDK subscribes to `system.shutdown` internally. When the Emergent engine
signals a graceful shutdown, active message streams close automatically.

## Helper Functions

`runSource`, `runHandler`, and `runSink` eliminate connection and
signal-handling boilerplate. Each helper connects, sets up SIGTERM/SIGINT
handlers, runs your callback, and disconnects on completion:

```typescript
import {
  createMessage,
  runHandler,
  runSink,
  runSource,
} from "jsr:@govcraft/emergent";

// Source -- custom event loop with shutdown signal
await runSource("my_source", async (source, shutdown) => {
  while (!shutdown.aborted) {
    await source.publish("tick", { time: Date.now() });
    await new Promise((r) => setTimeout(r, 1000));
  }
});

// Handler -- called once per message
await runHandler("my_handler", ["raw.event"], async (msg, handler) => {
  await handler.publish(
    createMessage("processed").causedBy(msg.id).payload({ done: true }),
  );
});

// Sink -- called once per message
await runSink("my_sink", ["timer.tick"], async (msg) => {
  console.log(msg.payload);
});
```

The name argument is optional. When omitted, the helper reads from the
`EMERGENT_NAME` environment variable.

`runSource` aborts `shutdown` on SIGTERM, on SIGINT, and, after SDK release
0.13.1, when the engine closes the connection. A Source subscribes to nothing,
so no stream ends to tell it the engine is gone, and a lost connection is the
only notice an engine that was killed ever gives. On 0.13.1 and earlier only the
two signals aborted it, so a Source that caught its publish errors kept running
against a dead socket unless the engine had spawned it on Linux, where the
kernel signals the child when its parent dies. `runHandler` and `runSink` end
with their message stream, which a lost connection closes.

## Error Handling

All SDK errors extend `EmergentError` and include a machine-readable `code`
property. Catch specific error types for precise control:

```typescript
import {
  ConnectionError,
  EmergentSource,
  SocketNotFoundError,
  TimeoutError,
} from "jsr:@govcraft/emergent";

try {
  const source = await EmergentSource.connect("my_source");
} catch (err) {
  if (err instanceof SocketNotFoundError) {
    console.error(`Engine not running at: ${err.socketPath}`);
  } else if (err instanceof TimeoutError) {
    console.error(`Timed out after ${err.timeoutMs}ms`);
  } else if (err instanceof ConnectionError) {
    console.error(`Connection failed: ${err.message}`);
  }
}
```

### Error Types

| Error                 | Code                  | Extra Fields   | Thrown when                                                                     |
| --------------------- | --------------------- | -------------- | ------------------------------------------------------------------------------- |
| `ConnectionError`     | `CONNECTION_FAILED`   |                | The socket cannot be reached, a request cannot be sent, or the client closes    |
| `SocketNotFoundError` | `SOCKET_NOT_FOUND`    | `socketPath`   | No socket file exists at the path                                               |
| `TimeoutError`        | `TIMEOUT`             | `timeoutMs`    | The engine does not answer a request in time                                    |
| `ProtocolError`       | `PROTOCOL_ERROR`      |                | A frame cannot be encoded, or a discovery reply is malformed                    |
| `SubscriptionError`   | `SUBSCRIPTION_FAILED` | `messageTypes` | The engine rejects a subscription, including the one a topology query makes     |
| `PublishError`        | `PUBLISH_FAILED`      | `messageType`  | The broker rejects an acknowledged publish, or the socket refuses a `publish()` |
| `DiscoveryError`      | `DISCOVERY_FAILED`    |                | The engine rejects `discover()`                                                 |
| `DisposedError`       | `DISPOSED`            |                | A closed client is used                                                         |
| `ValidationError`     | `VALIDATION_ERROR`    | `field`        | A message or a subscription topic is not valid                                  |

`SubscriptionError`, `PublishError` and `DiscoveryError` extend
`ConnectionError`. On SDK release 0.13.1 and earlier they were exported and
never thrown: each of those rejections threw a plain `ConnectionError` with the
code `CONNECTION_FAILED`. A handler that catches `ConnectionError` still catches
them, so test for the specific class first. A handler that compares `code`
against `CONNECTION_FAILED` for one of these rejections now sees the code in the
table.

When the socket refuses a write, the error keeps the socket's own error as
`cause`. After SDK release 0.13.1 a failed `publish()` throws a `PublishError`
whose `cause` is the `Deno.errors.*` error. On 0.13.1 and earlier `publish()`
threw that `Deno.errors.*` error itself, which is not an `EmergentError`, so a
handler written against the classes above never saw it. Code that tested for
`Deno.errors.BrokenPipe` on `publish()` should now test `err.cause`.

When the engine closes the connection, every call still waiting for an answer
(`publishAck()`, `discover()`, `subscribe()`, `getTopology()`,
`getMySubscriptions()`) rejects with `ConnectionError("Connection closed")` at
once, and the message stream ends, so a `for await` over it stops. That holds
after SDK release 0.13.1. On 0.13.1 and earlier each call waited out its own
timeout, 30 seconds by default, and then rejected with `TimeoutError`, and the
stream never ended, so a `for await` consumer waited until the process was
stopped.

A frame the engine sends with a malformed body is never thrown to the caller.
After SDK release 0.13.1 it is logged and skipped, and the subscription stays
open. On 0.13.1 and earlier one such frame ended the read loop and closed the
stream.

After SDK release 0.13.1 every frame is written to the socket in full, and
frames are written one at a time, so two concurrent publishes cannot interleave.
On 0.13.1 and earlier the SDK called `Deno.Conn.write` once per frame and
ignored the byte count, so a socket that took only part of a large frame left
half a frame on the wire and the engine lost framing for the rest of the
connection.

## Message Shape

Every message flowing through Emergent follows the same envelope:

| Field           | Type                                   | Description                        |
| --------------- | -------------------------------------- | ---------------------------------- |
| `id`            | `string`                               | Unique TypeID (`msg_<uuidv7>`)     |
| `messageType`   | `string`                               | Routing key (e.g., `"timer.tick"`) |
| `source`        | `string`                               | Name of the publishing primitive   |
| `correlationId` | `string \| undefined`                  | Links related messages             |
| `causationId`   | `string \| undefined`                  | ID of the triggering message       |
| `timestampMs`   | `number`                               | Creation time (Unix ms)            |
| `payload`       | `unknown`                              | User-defined data                  |
| `metadata`      | `Record<string, unknown> \| undefined` | Optional tracing/debug data        |

Use `msg.payloadAs<T>()` to access the payload with type safety.

## System Events

The Emergent engine broadcasts lifecycle events that your primitives can
subscribe to:

| Event Pattern           | Payload Type            | Fired When           |
| ----------------------- | ----------------------- | -------------------- |
| `system.started.<name>` | `SystemEventPayload`    | Primitive started    |
| `system.stopped.<name>` | `SystemEventPayload`    | Primitive stopped    |
| `system.error.<name>`   | `SystemEventPayload`    | Primitive failed     |
| `system.shutdown`       | `SystemShutdownPayload` | Engine shutting down |

Type guards are available for runtime checking:

```typescript
import { isErrorEvent, isSystemEventPayload } from "jsr:@govcraft/emergent";

if (isSystemEventPayload(msg.payload)) {
  if (isErrorEvent(msg.payload)) {
    console.error(`${msg.payload.name} failed: ${msg.payload.error}`);
  }
}
```

## Requirements

- Deno 2.x or later
- A running Emergent engine with the `EMERGENT_SOCKET` environment variable set
- Deno permissions: `--allow-env --allow-read --allow-write --allow-net=unix`

## License

MIT OR Apache-2.0
