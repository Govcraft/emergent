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
import { EmergentSink, EmergentSource, EmergentHandler } from "@govcraft/emergent";
```

Or import directly without a local install:

```typescript
import { EmergentSink } from "jsr:@govcraft/emergent";
```

## Three Primitives

Every Emergent workflow is composed of Sources, Handlers, and Sinks. Each
primitive has a single, well-defined role:

| Primitive     | Subscribe | Publish | Role                                         |
| ------------- | --------- | ------- | -------------------------------------------- |
| **Source**    | --        | Yes     | Ingress -- bring data into the system        |
| **Handler**   | Yes       | Yes     | Processing -- transform, enrich, or route    |
| **Sink**      | Yes       | --      | Egress -- persist, display, or forward data  |

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

A Handler subscribes to incoming messages and publishes new ones. Use
`causedBy` to link output messages to the input that triggered them:

```typescript
import { EmergentHandler, createMessage } from "jsr:@govcraft/emergent";

await using handler = await EmergentHandler.connect("order_processor");
const stream = await handler.subscribe(["order.created"]);

for await (const msg of stream) {
  await handler.publish(
    createMessage("order.processed")
      .causedBy(msg.id)
      .payload({ status: "ok" })
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
  createMessage("timer.tick").payload({ count: 1 })
);

// Full EmergentMessage object
await source.publish(message);
```

## Streaming Publish

Publish a collection or async stream of messages. Each message is sent
individually so subscribers begin consuming immediately. Both methods return
the count of successfully published messages and stop on the first error.

```typescript
// From an array or any Iterable
const messages = records.map(r =>
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

`runSource`, `runHandler`, and `runSink` eliminate connection and signal-handling
boilerplate. Each helper connects, sets up SIGTERM/SIGINT handlers, runs your
callback, and disconnects on completion:

```typescript
import { runSource, runHandler, runSink, createMessage } from "jsr:@govcraft/emergent";

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
    createMessage("processed").causedBy(msg.id).payload({ done: true })
  );
});

// Sink -- called once per message
await runSink("my_sink", ["timer.tick"], async (msg) => {
  console.log(msg.payload);
});
```

The name argument is optional. When omitted, the helper reads from the
`EMERGENT_NAME` environment variable.

## Error Handling

All SDK errors extend `EmergentError` and include a machine-readable `code`
property. Catch specific error types for precise control:

```typescript
import {
  EmergentSource,
  SocketNotFoundError,
  ConnectionError,
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

| Error                 | Code                  | Extra Fields     |
| --------------------- | --------------------- | ---------------- |
| `ConnectionError`     | `CONNECTION_FAILED`   |                  |
| `SocketNotFoundError` | `SOCKET_NOT_FOUND`    | `socketPath`     |
| `TimeoutError`        | `TIMEOUT`             | `timeoutMs`      |
| `ProtocolError`       | `PROTOCOL_ERROR`      |                  |
| `SubscriptionError`   | `SUBSCRIPTION_FAILED` | `messageTypes`   |
| `PublishError`        | `PUBLISH_FAILED`      | `messageType`    |
| `DiscoveryError`      | `DISCOVERY_FAILED`    |                  |
| `DisposedError`       | `DISPOSED`            |                  |
| `ValidationError`     | `VALIDATION_ERROR`    | `field`          |

## Message Shape

Every message flowing through Emergent follows the same envelope:

| Field            | Type                          | Description                            |
| ---------------- | ----------------------------- | -------------------------------------- |
| `id`             | `string`                      | Unique TypeID (`msg_<uuidv7>`)         |
| `messageType`    | `string`                      | Routing key (e.g., `"timer.tick"`)     |
| `source`         | `string`                      | Name of the publishing primitive       |
| `correlationId`  | `string \| undefined`         | Links related messages                 |
| `causationId`    | `string \| undefined`         | ID of the triggering message           |
| `timestampMs`    | `number`                      | Creation time (Unix ms)                |
| `payload`        | `unknown`                     | User-defined data                      |
| `metadata`       | `Record<string, unknown> \| undefined` | Optional tracing/debug data   |

Use `msg.payloadAs<T>()` to access the payload with type safety.

## System Events

The Emergent engine broadcasts lifecycle events that your primitives can
subscribe to:

| Event Pattern              | Payload Type           | Fired When                    |
| -------------------------- | ---------------------- | ----------------------------- |
| `system.started.<name>`    | `SystemEventPayload`   | Primitive started             |
| `system.stopped.<name>`    | `SystemEventPayload`   | Primitive stopped             |
| `system.error.<name>`      | `SystemEventPayload`   | Primitive failed              |
| `system.shutdown`          | `SystemShutdownPayload`| Engine shutting down          |

Type guards are available for runtime checking:

```typescript
import { isSystemEventPayload, isErrorEvent } from "jsr:@govcraft/emergent";

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
