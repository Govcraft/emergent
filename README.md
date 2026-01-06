# Emergent

A lightweight event-driven workflow engine with three simple primitives.

Building event-driven systems means coordinating processes that emit, transform, and consume events. Traditional workflow engines add complexity; rolling your own is fragile. Emergent gives you three primitives—Sources emit events, Handlers transform them, Sinks consume them—while the engine handles routing, lifecycle, and observability.

## Quick Example

A timer emits ticks every 5 seconds. A filter passes every 5th tick. A console prints the result.

```toml
# config/emergent.toml
[engine]
name = "emergent"
socket_path = "auto"

[[sources]]
name = "timer"
path = "./target/release/timer"
args = ["--interval", "5000"]
publishes = ["timer.tick"]

[[handlers]]
name = "filter"
path = "./target/release/filter"
args = ["--filter-every", "5"]
subscribes = ["timer.tick"]
publishes = ["timer.filtered"]

[[sinks]]
name = "console"
path = "./target/release/console"
subscribes = ["timer.filtered"]
```

Run the pipeline:

```bash
cargo build --release
./target/release/emergent --config ./config/emergent.toml
```

## The Three Primitives

```
Source ──publish──> Handler ──transform──> Sink
  │                    │                     │
  └── publish only ────┴── sub + publish ────┴── subscribe only
```

| Primitive | Subscribe | Publish | Purpose |
|-----------|-----------|---------|---------|
| Source    | No        | Yes     | Ingress: emit events into the system |
| Handler   | Yes       | Yes     | Transform: process and re-emit events |
| Sink      | Yes       | No      | Egress: consume events (logs, HTTP, etc.) |

## Features

- **Language-agnostic primitives**: Write in Rust, TypeScript, or Python—primitives are standalone processes
- **Built-in event sourcing**: Every message logged with causation chains for debugging and replay
- **Graceful lifecycle management**: Engine handles startup ordering, subscription routing, three-phase shutdown
- **Simple IPC protocol**: MessagePack over Unix sockets—no distributed systems setup
- **TOML configuration**: Declare your pipeline topology in one readable file

## Quick Start

```bash
# Clone and build
git clone https://github.com/emergent/emergent
cd emergent && cargo build --release

# Run the example pipeline
./target/release/emergent --config ./config/emergent.toml
```

## Writing a Source

```rust
use emergent_client::{EmergentSource, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = EmergentSource::connect("my_source").await?;

    let message = EmergentMessage::new("sensor.reading")
        .with_payload(json!({"temperature": 72.5}));

    source.publish(message).await?;
    Ok(())
}
```

## Writing a Handler

```rust
use emergent_client::{EmergentHandler, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let handler = EmergentHandler::connect("my_handler").await?;
    let mut stream = handler.subscribe(["sensor.reading"]).await?;

    while let Some(msg) = stream.next().await {
        let temp: f64 = msg.payload()["temperature"].as_f64().unwrap_or(0.0);

        if temp > 80.0 {
            let alert = EmergentMessage::new("alert.high_temp")
                .with_causation_id(msg.id())
                .with_payload(json!({"temperature": temp}));
            handler.publish(alert).await?;
        }
    }
    Ok(())
}
```

## Documentation

- **[Getting Started](docs/getting-started.md)** - Build your first pipeline in 30 minutes
- **[Concepts](docs/concepts.md)** - Architecture, message flow, event sourcing
- **[Primitives](docs/primitives/)** - Reference for Sources, Handlers, Sinks
- **[Configuration](docs/configuration.md)** - All configuration options
- **[SDKs](docs/sdks/)** - Rust, TypeScript, Python

## Requirements

- Rust 1.75+ (for engine and Rust primitives)
- Or: Deno 1.40+ (for TypeScript primitives)
- Or: Python 3.11+ with uv (for Python primitives)

## License

MIT
