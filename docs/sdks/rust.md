# Rust SDK

The `emergent-client` crate provides the Rust SDK for building Sources, Handlers, and Sinks.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
emergent-client = { path = "../emergent/sdks/rust" }
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

Or with cargo:

```bash
cargo add emergent-client tokio serde_json --features tokio/full
```

## Core Types

### EmergentMessage

The universal message envelope:

```rust
use emergent_client::EmergentMessage;
use serde_json::json;

// Create a message
let msg = EmergentMessage::new("event.type")
    .with_payload(json!({"key": "value"}))
    .with_source("my_source");

// With causation tracking
let response = EmergentMessage::new("event.processed")
    .with_causation_id(original_msg.id())
    .with_payload(json!({"result": "ok"}));

// With metadata
let msg = EmergentMessage::new("event.type")
    .with_payload(json!({"data": 42}))
    .with_metadata(json!({"trace_id": "abc123"}));

// With correlation ID
let msg = EmergentMessage::new("api.response")
    .with_correlation_id("request-123")
    .with_payload(json!({"status": "ok"}));
```

### Message Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique ID (`msg_<UUIDv7>`) |
| `message_type` | `String` | Type for subscription matching |
| `source` | `String` | Primitive that emitted this |
| `correlation_id` | `Option<String>` | Groups related messages |
| `causation_id` | `Option<String>` | ID of triggering message |
| `timestamp_ms` | `u64` | Unix timestamp in milliseconds |
| `payload` | `serde_json::Value` | Application data |
| `metadata` | `Option<serde_json::Value>` | Optional routing/debug info |

### Payload Extraction

```rust
// Get raw payload
let raw = msg.payload();

// Deserialize to typed struct
#[derive(Deserialize)]
struct MyPayload {
    count: u32,
    name: String,
}

let payload: MyPayload = msg.payload_as()?;
```

## EmergentSource

Sources publish messages into the system.

```rust
use emergent_client::{EmergentSource, EmergentMessage};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_source".to_string());

    let source = EmergentSource::connect(&name).await?;

    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut count = 0u64;

    loop {
        interval.tick().await;
        count += 1;

        let message = EmergentMessage::new("counter.tick")
            .with_payload(json!({"count": count}));

        source.publish(message).await?;
    }
}
```

### Source API

| Method | Description |
|--------|-------------|
| `connect(name)` | Connect to engine |
| `publish(message)` | Send a message (fire-and-forget) |
| `discover()` | Query available message types and primitives |
| `disconnect()` | Graceful disconnection |
| `name()` | Get this source's name |

## EmergentHandler

Handlers subscribe to messages and publish new ones.

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
        // Process the message
        let input: i64 = msg.payload()["value"].as_i64().unwrap_or(0);
        let output = input * 2;

        // Publish result with causation link
        let result = EmergentMessage::new("output.event")
            .with_causation_id(msg.id())
            .with_payload(json!({"doubled": output}));

        handler.publish(result).await?;
    }

    Ok(())
}
```

### Handler API

| Method | Description |
|--------|-------------|
| `connect(name)` | Connect to engine |
| `subscribe(types)` | Subscribe and get message stream |
| `publish(message)` | Send a message |
| `unsubscribe(types)` | Remove subscriptions |
| `discover()` | Query available message types |
| `disconnect()` | Graceful disconnection |
| `subscribed_types()` | Get current subscriptions |

### Subscription Flexibility

```rust
// Single topic (string)
let stream = handler.subscribe("timer.tick").await?;

// Multiple topics (array)
let stream = handler.subscribe(["timer.tick", "sensor.reading"]).await?;

// From Vec
let topics = vec!["timer.tick".to_string()];
let stream = handler.subscribe(topics).await?;

// Slice of &str
let topics: &[&str] = &["timer.tick", "sensor.reading"];
let stream = handler.subscribe(topics).await?;
```

## EmergentSink

Sinks subscribe to messages for output.

```rust
use emergent_client::EmergentSink;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_sink".to_string());

    let sink = EmergentSink::connect(&name).await?;

    // Get subscriptions from engine config
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

### Sink API

| Method | Description |
|--------|-------------|
| `connect(name)` | Connect to engine |
| `messages(name, types)` | Connect, get config subscriptions, return stream |
| `subscribe(types)` | Subscribe and get message stream |
| `get_my_subscriptions()` | Query configured subscriptions from engine |
| `unsubscribe(types)` | Remove subscriptions |
| `discover()` | Query available message types |
| `disconnect()` | Graceful disconnection |

### Convenience Method

```rust
// One-liner for simple sinks
let mut stream = EmergentSink::messages("console", ["timer.tick"]).await?;

while let Some(msg) = stream.next().await {
    println!("{}", msg.payload());
}
```

## MessageStream

The subscription stream:

```rust
let mut stream = handler.subscribe(["events"]).await?;

// Iterate with next()
while let Some(msg) = stream.next().await {
    process(msg);
}

// Stream ends on graceful shutdown
```

## Error Handling

```rust
use emergent_client::ClientError;

match source.publish(message).await {
    Ok(_) => println!("Sent"),
    Err(ClientError::SocketNotFound(path)) => {
        eprintln!("Socket not found: {}", path);
    }
    Err(ClientError::ConnectionFailed(msg)) => {
        eprintln!("Connection failed: {}", msg);
    }
    Err(ClientError::Timeout) => {
        eprintln!("Operation timed out");
    }
    Err(e) => eprintln!("Error: {:?}", e),
}
```

### Error Types

| Error | Description |
|-------|-------------|
| `SocketNotFound` | Engine socket doesn't exist |
| `ConnectionFailed` | Failed to connect to engine |
| `Timeout` | Operation timed out |
| `SubscriptionFailed` | Subscription request failed |
| `DiscoveryFailed` | Discovery request failed |
| `ProtocolError` | Unexpected protocol message |

## Graceful Shutdown

Handle SIGTERM and SDK shutdown signals:

```rust
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let handler = EmergentHandler::connect("my_handler").await?;
    let mut stream = handler.subscribe(["events"]).await?;
    let mut sigterm = signal(SignalKind::terminate())?;

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                handler.disconnect().await?;
                break;
            }
            msg = stream.next() => {
                match msg {
                    Some(msg) => process(msg).await?,
                    None => break,  // Graceful shutdown from engine
                }
            }
        }
    }

    Ok(())
}
```

## Discovery

Query available message types and primitives:

```rust
let info = source.discover().await?;

println!("Message types:");
for msg_type in &info.message_types {
    println!("  - {}", msg_type);
}

println!("Primitives:");
for primitive in &info.primitives {
    println!("  - {} ({})", primitive.name, primitive.kind);
}
```

## Serialization

Messages support both JSON and MessagePack:

```rust
// JSON
let json_bytes = message.to_json()?;
let from_json = EmergentMessage::from_json(&json_bytes)?;

// MessagePack
let msgpack_bytes = message.to_msgpack()?;
let from_msgpack = EmergentMessage::from_msgpack(&msgpack_bytes)?;
```

## See Also

- [Sources](../primitives/sources.md) - Building data ingress
- [Handlers](../primitives/handlers.md) - Building transformations
- [Sinks](../primitives/sinks.md) - Building data egress
- [Configuration](../configuration.md) - TOML reference
