# Sources

Sources are the ingress point for data entering an Emergent pipeline. They can only publish messages—they cannot subscribe to receive messages from other primitives.

## When to Use a Source

Use a Source when you need to bring external data into the system:

- **Timers**: Emit periodic events
- **HTTP webhooks**: Receive external HTTP requests
- **File watchers**: React to filesystem changes
- **Message queues**: Bridge from Kafka, RabbitMQ, etc.
- **APIs**: Poll external services

## Basic Structure

```rust
use emergent_client::{EmergentSource, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get name from engine (or use default for testing)
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_source".to_string());

    // Connect to engine
    let source = EmergentSource::connect(&name).await?;

    // Publish messages
    let message = EmergentMessage::new("my.event")
        .with_payload(json!({"data": "value"}));

    source.publish(message).await?;

    // Disconnect gracefully
    source.disconnect().await?;
    Ok(())
}
```

## Configuration

```toml
[[sources]]
name = "my_source"
path = "./target/release/my-source"
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

Handle SIGTERM for clean shutdown:

```rust
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = EmergentSource::connect("my_source").await?;
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

    Ok(())
}
```

## Patterns

### Polling Source

```rust
let mut interval = tokio::time::interval(Duration::from_secs(60));

loop {
    interval.tick().await;

    // Fetch from external API
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
3. **Error handling**: Sources should be resilient—log errors, don't crash
4. **Idempotency**: Include unique identifiers in payloads for deduplication downstream
5. **Silent operation**: Don't print to stdout/stderr—let Sinks handle output

## API Reference

### EmergentSource

| Method | Description |
|--------|-------------|
| `connect(name)` | Connect to engine |
| `publish(message)` | Send a message (fire-and-forget) |
| `discover()` | Query available message types |
| `disconnect()` | Graceful disconnection |
| `name()` | Get this source's name |

See also: [Handlers](handlers.md), [Sinks](sinks.md), [Rust SDK](../sdks/rust.md)
