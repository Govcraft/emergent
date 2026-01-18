# Handlers

Handlers are the transformation layer in an Emergent pipeline. They subscribe to messages, process them, and publish new messages. This is where your business logic lives.

## When to Use a Handler

Use a Handler when you need to transform data:

- **Filtering**: Pass through only messages matching criteria
- **Enrichment**: Add data from external sources
- **Aggregation**: Combine multiple messages into one
- **Routing**: Emit different message types based on content
- **Validation**: Check data and emit success/failure events

## Basic Structure

```rust
use emergent_client::{EmergentHandler, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_handler".to_string());

    let handler = EmergentHandler::connect(&name).await?;

    // Subscribe to message types
    let mut stream = handler.subscribe(["input.event"]).await?;

    // Process messages
    while let Some(msg) = stream.next().await {
        // Transform and publish
        let output = EmergentMessage::new("output.event")
            .with_causation_id(msg.id())
            .with_payload(json!({"transformed": msg.payload()}));

        handler.publish(output).await?;
    }

    Ok(())
}
```

## Configuration

```toml
[[handlers]]
name = "my_handler"
path = "./target/release/my-handler"
args = ["--threshold", "50"]
enabled = true
subscribes = ["input.event", "other.input"]
publishes = ["output.event", "error.event"]
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier for this handler |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments |
| `enabled` | No | Set to `false` to disable (default: `true`) |
| `subscribes` | Yes | Message types to receive |
| `publishes` | Yes | Message types this handler will emit |

## Causation Tracking

Always set `causation_id` to link derived messages to their source:

```rust
while let Some(msg) = stream.next().await {
    // This message was caused by the input message
    let output = EmergentMessage::new("processed.event")
        .with_causation_id(msg.id())  // Links to parent
        .with_payload(process(msg.payload()));

    handler.publish(output).await?;
}
```

This enables tracing any event back through its entire origin chain.

## Patterns

### Filter Pattern

Pass through only messages matching criteria:

```rust
while let Some(msg) = stream.next().await {
    let value = msg.payload()["value"].as_i64().unwrap_or(0);

    if value > 50 {
        let output = EmergentMessage::new("value.high")
            .with_causation_id(msg.id())
            .with_payload(msg.payload().clone());

        handler.publish(output).await?;
    }
    // Messages not matching are simply not forwarded
}
```

### Map Pattern

Transform every message:

```rust
while let Some(msg) = stream.next().await {
    let input: InputData = msg.payload_as()?;

    let output_data = OutputData {
        original: input.value,
        doubled: input.value * 2,
        timestamp: Utc::now(),
    };

    let output = EmergentMessage::new("value.transformed")
        .with_causation_id(msg.id())
        .with_payload(json!(output_data));

    handler.publish(output).await?;
}
```

### Enrich Pattern

Add data from external sources:

```rust
while let Some(msg) = stream.next().await {
    let user_id = msg.payload()["user_id"].as_str().unwrap_or("");

    // Fetch additional data
    let user_data = fetch_user_profile(user_id).await?;

    let enriched = json!({
        "original": msg.payload(),
        "user": user_data,
    });

    let output = EmergentMessage::new("event.enriched")
        .with_causation_id(msg.id())
        .with_payload(enriched);

    handler.publish(output).await?;
}
```

### Split Pattern

One input produces multiple outputs:

```rust
while let Some(msg) = stream.next().await {
    let items: Vec<Item> = msg.payload_as()?;

    for item in items {
        let output = EmergentMessage::new("item.individual")
            .with_causation_id(msg.id())
            .with_correlation_id(msg.id())  // Group related items
            .with_payload(json!(item));

        handler.publish(output).await?;
    }
}
```

### Route Pattern

Emit different message types based on content:

```rust
while let Some(msg) = stream.next().await {
    let event_type = msg.payload()["type"].as_str().unwrap_or("unknown");

    let output_type = match event_type {
        "order" => "order.received",
        "payment" => "payment.received",
        "refund" => "refund.requested",
        _ => "event.unknown",
    };

    let output = EmergentMessage::new(output_type)
        .with_causation_id(msg.id())
        .with_payload(msg.payload().clone());

    handler.publish(output).await?;
}
```

## Graceful Shutdown

The SDK handles `system.shutdown` automatically—the stream closes gracefully:

```rust
// Stream will end when engine sends system.shutdown
while let Some(msg) = stream.next().await {
    // Process...
}
// Clean exit here

// For additional cleanup, handle SIGTERM
let mut sigterm = signal(SignalKind::terminate())?;

loop {
    tokio::select! {
        _ = sigterm.recv() => {
            handler.disconnect().await?;
            break;
        }
        msg = stream.next() => {
            match msg {
                Some(msg) => { /* process */ }
                None => break,  // Graceful shutdown
            }
        }
    }
}
```

## Subscription Patterns

```rust
// Single topic
let stream = handler.subscribe("timer.tick").await?;

// Multiple topics (array)
let stream = handler.subscribe(["timer.tick", "sensor.reading"]).await?;

// From a Vec
let topics = vec!["timer.tick".to_string()];
let stream = handler.subscribe(topics).await?;
```

## Best Practices

1. **Always set causation_id**: This enables event tracing
2. **Fail gracefully**: Handle parse errors, don't crash
3. **Keep handlers focused**: One handler, one transformation
4. **Silent operation**: Don't print to stdout—let Sinks handle output
5. **Idempotent processing**: Same input should produce same output

## API Reference

### EmergentHandler

| Method | Description |
|--------|-------------|
| `connect(name)` | Connect to engine |
| `subscribe(types)` | Subscribe and get message stream |
| `publish(message)` | Send a message |
| `unsubscribe(types)` | Remove subscriptions |
| `discover()` | Query available message types |
| `disconnect()` | Graceful disconnection |
| `subscribed_types()` | Get current subscriptions |

See also: [Sources](sources.md), [Sinks](sinks.md), [Rust SDK](../sdks/rust.md)
