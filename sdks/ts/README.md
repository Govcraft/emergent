# Emergent TypeScript SDK

TypeScript/Deno SDK for the Emergent event-driven workflow platform.

## Installation

```bash
deno add jsr:@govcraft/emergent
```

Then import:

```typescript
import { EmergentSink, EmergentSource, EmergentHandler } from "@govcraft/emergent";
```

Or import directly without installing:

```typescript
import { EmergentSink } from "jsr:@govcraft/emergent";
```

## Quick Start

### Sink (Subscribe Only) - 3 Lines

```typescript
import { EmergentSink } from "jsr:@govcraft/emergent";

for await (const msg of EmergentSink.messages("my_sink", ["timer.tick"])) {
  console.log(msg.payload);
}
```

### Source (Publish Only)

```typescript
import { EmergentSource } from "jsr:@govcraft/emergent";

await using source = await EmergentSource.connect("my_source");
await source.publish("sensor.reading", { value: 42.5, unit: "celsius" });
```

### Handler (Subscribe + Publish)

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

## Three Primitives

| Primitive | Can Subscribe | Can Publish | Use Case |
|-----------|--------------|-------------|----------|
| **Source** | No | Yes | Ingress: HTTP endpoints, sensors, external APIs |
| **Handler** | Yes | Yes | Processing: transform, enrich, route data |
| **Sink** | Yes | No | Egress: database writers, notifications, external APIs |

## Publish Overloads

All three forms are equivalent:

```typescript
// Shorthand (type + payload)
await source.publish("timer.tick", { count: 1 });

// MessageBuilder (fluent API)
await source.publish(
  createMessage("timer.tick").payload({ count: 1 })
);

// Complete EmergentMessage
await source.publish(message);
```

## Message Building

```typescript
import { createMessage } from "jsr:@govcraft/emergent";

// Simple message
const msg = createMessage("timer.tick").payload({ count: 1 }).build();

// With causation tracking
const reply = createMessage("order.confirmed")
  .causedBy(originalMsg.id)
  .payload({ confirmed: true })
  .build();

// With all options
const msg = createMessage("sensor.reading")
  .payload({ value: 42.5, unit: "celsius" })
  .metadata({ sensor_id: "temp-01", location: "room-a" })
  .source("sensor_service")
  .build();
```

## Subscribe Patterns

```typescript
// Array form
const stream = await sink.subscribe(["timer.tick", "timer.filtered"]);

// Variadic form
const stream = await sink.subscribe("timer.tick", "timer.filtered");

// Iterate
for await (const msg of stream) {
  const data = msg.payloadAs<{ count: number }>();
  console.log(data.count);
}
```

## Resource Cleanup

All primitives implement `Disposable` and `AsyncDisposable`:

```typescript
// Automatic cleanup with await using
await using sink = await EmergentSink.connect("my_sink");

// Automatic cleanup with using
using source = await EmergentSource.connect("my_source");

// Manual cleanup
const handler = await EmergentHandler.connect("my_handler");
// ... use handler ...
handler.close();
```

## Helper Functions

Simplified lifecycle management with signal handling:

```typescript
import { runSource, runHandler, runSink } from "jsr:@govcraft/emergent";

// Source helper
await runSource("my_source", async (source, shutdown) => {
  while (!shutdown.aborted) {
    await source.publish("tick", { time: Date.now() });
    await new Promise((r) => setTimeout(r, 1000));
  }
});

// Handler helper
await runHandler("my_handler", ["raw.event"], async (msg, handler) => {
  await handler.publish(
    createMessage("processed").causedBy(msg.id).payload({ done: true })
  );
});

// Sink helper
await runSink("my_sink", ["timer.tick"], async (msg) => {
  console.log(msg.payload);
});
```

## Error Handling

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

| Error | Code | Extra Fields |
|-------|------|-------------|
| `ConnectionError` | `CONNECTION_FAILED` | |
| `SocketNotFoundError` | `SOCKET_NOT_FOUND` | `socketPath` |
| `TimeoutError` | `TIMEOUT` | `timeoutMs` |
| `ProtocolError` | `PROTOCOL_ERROR` | |
| `SubscriptionError` | `SUBSCRIPTION_FAILED` | `messageTypes` |
| `PublishError` | `PUBLISH_FAILED` | `messageType` |
| `DiscoveryError` | `DISCOVERY_FAILED` | |
| `DisposedError` | `DISPOSED` | |
| `ValidationError` | `VALIDATION_ERROR` | `field` |

## Requirements

- Deno 2.x+
- Running Emergent engine with `EMERGENT_SOCKET` environment variable set
- Deno permissions: `--allow-env --allow-read --allow-write --allow-net=unix`

## License

MIT OR Apache-2.0
