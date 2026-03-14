# TypeScript SDK

The `@govcraft/emergent` package provides the TypeScript/Deno SDK for building Sources, Handlers, and Sinks.

## Installation

```bash
deno add jsr:@govcraft/emergent
```

Or import directly without installing:

```typescript
import { EmergentSink } from "jsr:@govcraft/emergent";
```

- **JSR**: [@govcraft/emergent](https://jsr.io/@govcraft/emergent)

## Documentation

See the full SDK README for API reference, examples, and error handling:

- [TypeScript SDK README](../../sdks/ts/README.md)

## See Also

- [Rust SDK](rust.md) - crates.io: [emergent-client](https://crates.io/crates/emergent-client)
- [Python SDK](python.md) - PyPI: [emergent-client](https://pypi.org/project/emergent-client/)
- [Go SDK](go.md) - `go get github.com/govcraft/emergent/sdks/go`
- [Sources](../primitives/sources.md) - Building data ingress
- [Handlers](../primitives/handlers.md) - Building transformations
- [Sinks](../primitives/sinks.md) - Building data egress
- [Configuration](../configuration.md) - TOML reference
