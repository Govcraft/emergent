# Sinks

Sinks are the egress point for data leaving an Emergent pipeline. They subscribe to messages but cannot publish—they're the final destination for events.

## When to Use a Sink

Use a Sink when you need to output data:

- **Console/Logging**: Display events for monitoring
- **Databases**: Persist events to PostgreSQL, SQLite, etc.
- **Files**: Write to log files or data exports
- **HTTP webhooks**: Send to external services
- **Notifications**: Email, Slack, SMS alerts
- **Metrics**: Push to Prometheus, StatsD, etc.

## Basic Structure

```rust
use emergent_client::EmergentSink;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_sink".to_string());

    let sink = EmergentSink::connect(&name).await?;

    // Get subscriptions from engine config
    let topics = sink.get_my_subscriptions().await?;

    // Subscribe and process
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

## Configuration

```toml
[[sinks]]
name = "my_sink"
path = "./target/release/my-sink"
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

// Wildcards work in configuration
// subscribes = ["system.started.*"]  matches system.started.timer, etc.
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
                    // Graceful shutdown - flush remaining
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
            // Flush any buffers
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

Sinks should be resilient:

```rust
while let Some(msg) = stream.next().await {
    if let Err(e) = process(msg).await {
        eprintln!("Failed to process message: {}", e);
        // Log and continue—don't crash
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

This connects, queries configured subscriptions, and returns the stream.

## Best Practices

1. **Query subscriptions from engine**: Use `get_my_subscriptions()` for config-driven behavior
2. **Handle errors gracefully**: Log and continue, don't crash
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

See also: [Sources](sources.md), [Handlers](handlers.md), [Rust SDK](../sdks/rust.md)
