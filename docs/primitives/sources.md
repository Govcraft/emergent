# Sources

Sources are the ingress point for data entering an Emergent pipeline. They can only publish messages -- they cannot subscribe.

## Two Approaches

### Marketplace exec-source (zero code)

For most use cases, the marketplace `exec-source` turns any shell command into a Source. No code required:

```toml
[[sources]]
name = "timer"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "5000"]
publishes = ["exec.output"]
```

```toml
# One-shot: run once and exit
[[sources]]
name = "seed"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--shell", "sh", "--command", "echo '{\"pattern\":\"glider\"}'"]
publishes = ["life.seed"]
```

```toml
# Shell command with pipes
[[sources]]
name = "cpu"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--shell", "sh", "--command", "grep 'cpu ' /proc/stat | awk '{printf \"{\\\"idle\\\":%d,\\\"total\\\":%d}\", $5, $2+$3+$4+$5}'", "--interval", "1000"]
publishes = ["metric.cpu"]
```

Install with:

```bash
emergent marketplace install exec-source
```

### Custom SDK Source (when exec is not enough)

Write a custom Source when you need persistent connections, custom protocols, or complex async logic (HTTP servers, file watchers, message queue bridges).

## When to Use a Custom Source

- **HTTP servers**: Accept webhooks with custom routing or authentication
- **File watchers**: React to filesystem changes with debouncing
- **Message queue bridges**: Consume from Kafka, RabbitMQ, NATS
- **Streaming connections**: Maintain long-lived connections (SSE, gRPC streams)
- **Custom protocols**: Binary or proprietary protocols

For periodic command execution, polling APIs via curl, or one-shot data emission, use exec-source instead.

## Basic Structure (Rust)

```rust
use emergent_client::{EmergentSource, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_source".to_string());

    let source = EmergentSource::connect(&name).await?;

    let message = EmergentMessage::new("my.event")
        .with_payload(json!({"data": "value"}));

    source.publish(message).await?;
    source.disconnect().await?;
    Ok(())
}
```

Or use the helper for less boilerplate:

```rust
use emergent_client::helpers::run_source;
use emergent_client::EmergentMessage;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_source(Some("my_source"), |source, mut shutdown| async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = interval.tick() => {
                    let msg = EmergentMessage::new("my.event")
                        .with_payload(json!({"count": 1}));
                    source.publish(msg).await.map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }).await?;
    Ok(())
}
```

## Configuration

```toml
[[sources]]
name = "my_source"
path = "/path/to/my-source"
args = ["--interval", "1000"]
enabled = true
publishes = ["my.event", "my.other_event"]
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier for this source |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments |
| `enabled` | No | Set to `false` to disable (default: `true`) |
| `publishes` | Yes | Message types this source will emit |

The `path` can be any executable: a compiled binary, a script run through an interpreter (`"deno"`, `"python3"`), or a marketplace binary.

## Message Construction

```rust
// Basic message
let msg = EmergentMessage::new("sensor.reading")
    .with_payload(json!({"temperature": 72.5}));

// With metadata
let msg = EmergentMessage::new("sensor.reading")
    .with_payload(json!({"temperature": 72.5}))
    .with_metadata(json!({"sensor_id": "A1", "location": "room1"}));

// With correlation ID (for request-response patterns)
let msg = EmergentMessage::new("api.response")
    .with_correlation_id("req_12345")
    .with_payload(json!({"status": "ok"}));
```

## Graceful Shutdown

Sources receive SIGTERM from the engine during shutdown (Phase 1). The helper handles this automatically. For manual control:

```rust
use tokio::signal::unix::{signal, SignalKind};

let mut sigterm = signal(SignalKind::terminate())?;
let mut interval = tokio::time::interval(Duration::from_secs(1));

loop {
    tokio::select! {
        _ = sigterm.recv() => {
            source.disconnect().await?;
            break;
        }
        _ = interval.tick() => {
            // Publish your message
        }
    }
}
```

## Patterns

### Polling Source

```rust
let mut interval = tokio::time::interval(Duration::from_secs(60));

loop {
    interval.tick().await;
    let data = fetch_external_api().await?;
    let message = EmergentMessage::new("api.data")
        .with_payload(data);
    source.publish(message).await?;
}
```

### HTTP Webhook Source

```rust
use axum::{routing::post, Router, Json};

async fn webhook_handler(
    source: EmergentSource,
    Json(payload): Json<serde_json::Value>,
) -> &'static str {
    let message = EmergentMessage::new("webhook.received")
        .with_payload(payload);
    source.publish(message).await.ok();
    "OK"
}
```

### File Watcher Source

```rust
use notify::{Watcher, RecursiveMode, watcher};

let (tx, rx) = std::sync::mpsc::channel();
let mut watcher = watcher(tx, Duration::from_secs(1))?;
watcher.watch("/path/to/watch", RecursiveMode::Recursive)?;

for event in rx {
    let message = EmergentMessage::new("file.changed")
        .with_payload(json!({"event": format!("{:?}", event)}));
    source.publish(message).await?;
}
```

## Best Practices

1. **Message type naming**: Use dot-separated namespaces (`domain.action`)
2. **Payload structure**: Keep payloads self-contained with all needed context
3. **Error handling**: Sources should be resilient -- log errors, do not crash
4. **Idempotency**: Include unique identifiers in payloads for deduplication downstream
5. **Silent operation**: Do not print to stdout/stderr -- let Sinks handle output

## API Reference

### EmergentSource

| Method | Description |
|--------|-------------|
| `connect(name)` | Connect to engine |
| `publish(message)` | Send a message (fire-and-forget) |
| `discover()` | Query available message types |
| `disconnect()` | Graceful disconnection |
| `name()` | Get this source's name |

See also: [Handlers](handlers.md), [Sinks](sinks.md), [Rust SDK](../sdks/rust.md), [TypeScript SDK](../sdks/typescript.md), [Python SDK](../sdks/python.md), [Go SDK](../sdks/go.md)
