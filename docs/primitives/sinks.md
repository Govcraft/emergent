# Sinks

Sinks are the egress point for data leaving an Emergent pipeline. They subscribe to messages but cannot publish -- they are the final destination for events.

## Two Approaches

### Marketplace exec-sink (zero code)

For most use cases, the marketplace `exec-sink` pipes event payloads through any executable. No code required:

```toml
# Pretty-print events with jq
[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "data.processed", "--", "jq", "."]
subscribes = ["data.processed"]

# Post to a Slack channel via curl
[[sinks]]
name = "slack-poster"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "alert.triggered", "--", "sh", "-c", "jq -c . | curl -s -X POST https://slack.com/api/chat.postMessage -H \"Authorization: Bearer $SLACK_BOT_TOKEN\" -H 'Content-Type: application/json' -d @-"]
subscribes = ["alert.triggered"]

# Append to a log file
[[sinks]]
name = "logger"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "event.processed", "--", "sh", "-c", "jq -c . >> /var/log/events.jsonl"]
subscribes = ["event.processed"]
```

Install with:

```bash
emergent marketplace install exec-sink
```

Other marketplace sinks:

```bash
emergent marketplace install sse-sink          # Push events to browsers via Server-Sent Events
emergent marketplace install topology-viewer   # Real-time D3.js pipeline visualization
```

### Custom SDK Sink (when exec is not enough)

Write a custom Sink when you need persistent connections (database pools, streaming uploads), batching, retry logic, or complex output formatting.

## When to Use a Custom Sink

- **Database writes**: Connection pooling, prepared statements, batch inserts
- **Streaming output**: Maintain long-lived connections to external services
- **Batching**: Collect messages and flush periodically for efficiency
- **Retry logic**: Exponential backoff on transient failures
- **Custom formatting**: Complex output transformations beyond what shell commands can express

For simple output (printing, curling an API, appending to files), use exec-sink instead.

## Basic Structure (Rust)

```rust
use emergent_client::EmergentSink;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_sink".to_string());

    let sink = EmergentSink::connect(&name).await?;
    let topics = sink.get_my_subscriptions().await?;
    let mut stream = sink.subscribe(&topics).await?;

    while let Some(msg) = stream.next().await {
        println!("[{}] {}: {}",
            msg.message_type(),
            msg.source(),
            msg.payload()
        );
    }

    Ok(())
}
```

Or use the helper:

```rust
use emergent_client::helpers::run_sink;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_sink(
        Some("my_sink"),
        &["event.processed"],
        |msg| async move {
            println!("{}", msg.payload());
            Ok(())
        }
    ).await?;
    Ok(())
}
```

## Configuration

```toml
[[sinks]]
name = "my_sink"
path = "/path/to/my-sink"
args = ["--output", "/var/log/events.log"]
enabled = true
subscribes = ["event.processed", "system.started.*", "system.error.*"]
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier for this sink |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments |
| `enabled` | No | Set to `false` to disable (default: `true`) |
| `subscribes` | Yes | Message types to receive |

## Subscription Patterns

```rust
// From engine configuration (recommended)
let topics = sink.get_my_subscriptions().await?;
let stream = sink.subscribe(&topics).await?;

// Or explicit topics
let stream = sink.subscribe(["event.type", "other.event"]).await?;
```

Wildcards work in configuration:

```toml
subscribes = ["system.started.*"]  # matches system.started.timer, etc.
```

## Patterns

### Console Sink

```rust
while let Some(msg) = stream.next().await {
    let timestamp = chrono::Utc::now().format("%H:%M:%S%.3f");
    println!("[{}] {} from {}: {}",
        timestamp,
        msg.message_type(),
        msg.source(),
        serde_json::to_string_pretty(msg.payload())?
    );
}
```

### File Logger Sink

```rust
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

let mut file = OpenOptions::new()
    .create(true)
    .append(true)
    .open("/var/log/events.jsonl")
    .await?;

while let Some(msg) = stream.next().await {
    let json = serde_json::to_string(&msg)?;
    file.write_all(format!("{}\n", json).as_bytes()).await?;
}
```

### Database Sink

```rust
use sqlx::PgPool;

let pool = PgPool::connect("postgres://localhost/events").await?;

while let Some(msg) = stream.next().await {
    sqlx::query!(
        "INSERT INTO events (id, message_type, payload, timestamp_ms) VALUES ($1, $2, $3, $4)",
        msg.id(),
        msg.message_type(),
        msg.payload(),
        msg.timestamp_ms as i64
    )
    .execute(&pool)
    .await?;
}
```

### HTTP Webhook Sink

```rust
let client = reqwest::Client::new();

while let Some(msg) = stream.next().await {
    client.post("https://api.example.com/webhook")
        .json(&msg)
        .send()
        .await?;
}
```

### Batching Sink

Collect messages and flush periodically:

```rust
let mut batch: Vec<EmergentMessage> = Vec::new();
let mut flush_interval = tokio::time::interval(Duration::from_secs(5));

loop {
    tokio::select! {
        msg = stream.next() => {
            match msg {
                Some(msg) => {
                    batch.push(msg);
                    if batch.len() >= 100 {
                        flush_batch(&batch).await?;
                        batch.clear();
                    }
                }
                None => {
                    if !batch.is_empty() {
                        flush_batch(&batch).await?;
                    }
                    break;
                }
            }
        }
        _ = flush_interval.tick() => {
            if !batch.is_empty() {
                flush_batch(&batch).await?;
                batch.clear();
            }
        }
    }
}
```

## Graceful Shutdown

The SDK handles `system.shutdown` automatically:

```rust
// Stream closes gracefully when engine shuts down
while let Some(msg) = stream.next().await {
    process(msg).await?;
}
// All messages consumed before exit
```

For additional cleanup:

```rust
let mut sigterm = signal(SignalKind::terminate())?;

loop {
    tokio::select! {
        _ = sigterm.recv() => {
            sink.disconnect().await?;
            break;
        }
        msg = stream.next() => {
            match msg {
                Some(msg) => process(msg).await?,
                None => break,
            }
        }
    }
}
```

## Error Handling

Sinks should be resilient -- log errors and continue processing:

```rust
while let Some(msg) = stream.next().await {
    if let Err(e) = process(msg).await {
        eprintln!("Failed to process message: {}", e);
    }
}
```

For critical sinks, consider retry logic:

```rust
async fn process_with_retry(msg: &EmergentMessage) -> Result<()> {
    let mut attempts = 0;
    loop {
        match process(msg).await {
            Ok(_) => return Ok(()),
            Err(e) if attempts < 3 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_secs(1 << attempts)).await;
            }
            Err(e) => return Err(e),
        }
    }
}
```

## Convenience API

For simple sinks, use the one-liner:

```rust
let mut stream = EmergentSink::messages("console", ["timer.tick"]).await?;

while let Some(msg) = stream.next().await {
    println!("{}", msg.payload());
}
```

## Best Practices

1. **Query subscriptions from engine**: Use `get_my_subscriptions()` for config-driven behavior
2. **Handle errors gracefully**: Log and continue, do not crash
3. **Flush on shutdown**: Ensure all buffered data is written
4. **Subscribe to system events**: Monitor `system.error.*` for debugging
5. **Sinks own output**: This is the only place to print, log, or send externally

## API Reference

### EmergentSink

| Method | Description |
|--------|-------------|
| `connect(name)` | Connect to engine |
| `messages(name, types)` | Connect, get config subscriptions, return stream |
| `subscribe(types)` | Subscribe and get message stream |
| `get_my_subscriptions()` | Query configured subscriptions from engine |
| `unsubscribe(types)` | Remove subscriptions |
| `discover()` | Query available message types |
| `disconnect()` | Graceful disconnection |

See also: [Sources](sources.md), [Handlers](handlers.md), [Rust SDK](../sdks/rust.md), [TypeScript SDK](../sdks/typescript.md), [Python SDK](../sdks/python.md), [Go SDK](../sdks/go.md)
