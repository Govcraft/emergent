# Handlers

Handlers are the transformation layer in an Emergent pipeline. They subscribe to messages, process them, and publish new messages. This is where your processing logic lives -- filtering, enrichment, model inference, routing, aggregation.

## Two Approaches

### Marketplace exec-handler (zero code)

For most use cases, the marketplace `exec-handler` pipes event payloads through any executable. The tool reads stdin (the event payload as JSON) and writes stdout (the new event payload). No code required:

```toml
# Data transformation with jq
[[handlers]]
name = "transform"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "input.event", "--publish-as", "output.event", "--", "jq", ".data | map(select(.score > 0.8))"]
subscribes = ["input.event"]
publishes = ["output.event"]

# LLM inference with Claude
[[handlers]]
name = "analyzer"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "data.raw", "--publish-as", "data.analyzed", "--timeout", "60000", "--", "claude", "-p", "Analyze this data and summarize key findings"]
subscribes = ["data.raw"]
publishes = ["data.analyzed"]

# Local model via Ollama
[[handlers]]
name = "classifier"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "text.input", "--publish-as", "text.classified", "--", "sh", "-c", "curl -s http://localhost:11434/api/generate -d '{\"model\":\"llama3\",\"prompt\":'$(cat)',\"stream\":false}' | jq .response"]
subscribes = ["text.input"]
publishes = ["text.classified"]

# Python ML model
[[handlers]]
name = "predictor"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "features.extracted", "--publish-as", "prediction.result", "--", "python3", "predict.py"]
subscribes = ["features.extracted"]
publishes = ["prediction.result"]
```

Install with:

```bash
emergent marketplace install exec-handler
```

The exec-handler is **stateless** -- each message spawns a fresh process. This is a feature: process isolation means a crashed model call cannot corrupt state or take down other handlers.

### Custom SDK Handler (when exec is not enough)

Write a custom Handler when you need:

- **Persistent state across messages**: Running averages, counters, in-memory caches, world state (like the Game of Life handler)
- **High-performance processing**: Eliminate per-message process spawn overhead
- **Custom protocols**: Binary protocols, streaming connections
- **Complex async logic**: Concurrent processing, backpressure management

## When to Use a Custom Handler

- **Stateful transformation**: Computing per-second deltas from cumulative counters (system-monitor), maintaining a simulation grid (game-of-life), tracking conversation context
- **Aggregation**: Collecting messages over a time window before emitting a summary
- **Complex routing**: Dynamic routing based on accumulated state
- **Performance-critical paths**: High-throughput message processing where per-message process spawn is too expensive

For stateless transformations (jq, model calls, data extraction), use exec-handler instead.

## Basic Structure (Rust)

```rust
use emergent_client::{EmergentHandler, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_handler".to_string());

    let handler = EmergentHandler::connect(&name).await?;
    let mut stream = handler.subscribe(["input.event"]).await?;

    while let Some(msg) = stream.next().await {
        let output = EmergentMessage::new("output.event")
            .with_causation_id(msg.id())
            .with_payload(json!({"transformed": msg.payload()}));

        handler.publish(output).await?;
    }

    Ok(())
}
```

Or use the helper:

```rust
use emergent_client::helpers::run_handler;
use emergent_client::EmergentMessage;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_handler(
        Some("my_handler"),
        &["input.event"],
        |msg, handler| async move {
            let output = EmergentMessage::new("output.event")
                .with_causation_from_message(msg.id())
                .with_payload(json!({"processed": true}));
            handler.publish(output).await.map_err(|e| e.to_string())
        }
    ).await?;
    Ok(())
}
```

## Configuration

```toml
[[handlers]]
name = "my_handler"
path = "/path/to/my-handler"
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
let output = EmergentMessage::new("processed.event")
    .with_causation_id(msg.id())
    .with_payload(process(msg.payload()));
```

This enables tracing any event back through its entire origin chain in the event store.

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
}
```

### Map Pattern

Transform every message:

```rust
while let Some(msg) = stream.next().await {
    let input: InputData = msg.payload_as()?;
    let output = EmergentMessage::new("value.transformed")
        .with_causation_id(msg.id())
        .with_payload(json!({"original": input.value, "doubled": input.value * 2}));
    handler.publish(output).await?;
}
```

### Enrich Pattern

Add data from external sources:

```rust
while let Some(msg) = stream.next().await {
    let user_id = msg.payload()["user_id"].as_str().unwrap_or("");
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
            .with_correlation_id(msg.id())
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

The SDK handles `system.shutdown` automatically -- the stream closes gracefully:

```rust
// Stream ends when engine sends system.shutdown
while let Some(msg) = stream.next().await {
    // Process...
}
// Clean exit here
```

For additional cleanup:

```rust
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

1. **Always set causation_id**: This enables event tracing through the event store
2. **Fail gracefully**: Handle parse errors, do not crash
3. **Keep handlers focused**: One handler, one transformation
4. **Silent operation**: Do not print to stdout -- let Sinks handle output
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

See also: [Sources](sources.md), [Sinks](sinks.md), [Rust SDK](../sdks/rust.md), [TypeScript SDK](../sdks/typescript.md), [Python SDK](../sdks/python.md), [Go SDK](../sdks/go.md)
