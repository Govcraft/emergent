# Python SDK

The `emergent-client` package provides the Python SDK for building custom Sources, Handlers, and Sinks. Use this when marketplace exec primitives are not enough -- you need persistent state across messages, custom protocols, or complex async logic.

For stateless transformations (jq, model calls, data extraction), use the marketplace exec primitives instead. See [Getting Started](../getting-started.md).

## Installation

```bash
pip install emergent-client
```

Or with uv:

```bash
uv add emergent-client
```

- **PyPI**: [emergent-client](https://pypi.org/project/emergent-client/)

## Quick Examples

### Sink (consume messages)

```python
from emergent import EmergentSink

async for msg in EmergentSink.messages("my_sink", ["timer.tick"]):
    print(msg.payload)
```

### Handler (transform messages)

```python
from emergent import run_handler, create_message

async def process(msg, handler):
    data = msg.payload_as(dict)
    enriched = {**data, "processed_by": "python"}
    await handler.publish(
        create_message("data.enriched")
        .caused_by(msg.id)
        .payload(enriched)
    )

import asyncio
asyncio.run(run_handler("enricher", ["data.raw"], process))
```

### Source (publish messages)

```python
from emergent import run_source, create_message

async def timer_logic(source, shutdown_event):
    while not shutdown_event.is_set():
        try:
            await asyncio.wait_for(shutdown_event.wait(), timeout=5.0)
            break
        except asyncio.TimeoutError:
            await source.publish(
                create_message("timer.tick").payload({"count": 1})
            )

import asyncio
asyncio.run(run_source("my_timer", timer_logic))
```

## Full Documentation

See the complete SDK README for the full API reference, advanced patterns, error handling, and configuration examples:

- [Python SDK README](../../sdks/py/README.md)

## See Also

- [Rust SDK](rust.md) - crates.io: [emergent-client](https://crates.io/crates/emergent-client)
- [TypeScript SDK](typescript.md) - JSR: [@govcraft/emergent](https://jsr.io/@govcraft/emergent)
- [Go SDK](go.md) - `go get github.com/govcraft/emergent/sdks/go`
- [Sources](../primitives/sources.md) - Building data ingress
- [Handlers](../primitives/handlers.md) - Building transformations
- [Sinks](../primitives/sinks.md) - Building data egress
- [Configuration](../configuration.md) - TOML reference
