# TypeScript SDK

The `@govcraft/emergent` package provides the TypeScript/Deno SDK for building custom Sources, Handlers, and Sinks. Use this when marketplace exec primitives are not enough -- you need persistent state across messages, custom protocols, or complex async logic.

For stateless transformations (jq, model calls, data extraction), use the marketplace exec primitives instead. See [Getting Started](../getting-started.md).

## Installation

```bash
deno add jsr:@govcraft/emergent
```

Or import directly without installing:

```typescript
import { EmergentSink } from "jsr:@govcraft/emergent";
```

- **JSR**: [@govcraft/emergent](https://jsr.io/@govcraft/emergent)

## Quick Examples

### Sink (consume messages)

```typescript
import { runSink } from "jsr:@govcraft/emergent";

await runSink("my_sink", ["timer.tick"], async (msg) => {
  const data = msg.payloadAs<{ count: number }>();
  console.log(`Tick #${data.count}`);
});
```

### Handler (transform messages)

```typescript
import { runHandler } from "jsr:@govcraft/emergent";

await runHandler("my_handler", ["data.raw"], async (msg, handler) => {
  const data = msg.payloadAs<{ value: number }>();
  await handler.publish({
    messageType: "data.processed",
    causationId: msg.id,
    payload: { doubled: data.value * 2 },
  });
});
```

### Source (publish messages)

```typescript
import { runSource } from "jsr:@govcraft/emergent";

await runSource("my_source", async (source, shutdown) => {
  const intervalId = setInterval(async () => {
    await source.publish({
      messageType: "timer.tick",
      payload: { timestamp: Date.now() },
    });
  }, 3000);

  await new Promise<void>((resolve) => {
    shutdown.addEventListener("abort", () => {
      clearInterval(intervalId);
      resolve();
    });
  });
});
```

## Full Documentation

See the complete SDK README for the full API reference, advanced patterns, error handling, and configuration examples:

- [TypeScript SDK README](../../sdks/ts/README.md)

## See Also

- [Rust SDK](rust.md) - crates.io: [emergent-client](https://crates.io/crates/emergent-client)
- [Python SDK](python.md) - PyPI: [emergent-client](https://pypi.org/project/emergent-client/)
- [Go SDK](go.md) - `go get github.com/govcraft/emergent/sdks/go`
- [Sources](../primitives/sources.md) - Building data ingress
- [Handlers](../primitives/handlers.md) - Building transformations
- [Sinks](../primitives/sinks.md) - Building data egress
- [Configuration](../configuration.md) - TOML reference
