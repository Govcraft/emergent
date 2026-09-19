# Rust SDK

The `emergent-client` crate provides the Rust SDK for building custom Sources, Handlers, and Sinks. Use this when marketplace exec primitives are not enough -- you need persistent state across messages, custom protocols, or high-performance processing.

For stateless transformations (jq, model calls, data extraction), use the marketplace exec primitives instead. See [Getting Started](../getting-started.md).

## Installation

```bash
cargo add emergent-client tokio serde_json --features tokio/full
```

- **crates.io**: [emergent-client](https://crates.io/crates/emergent-client)
- **docs.rs**: [emergent-client](https://docs.rs/emergent-client)

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

## Helper Functions

Helpers eliminate boilerplate for the common case -- connection, signal handling, and graceful shutdown are automatic:

```rust
use emergent_client::helpers::{run_source, run_handler, run_sink};

// Source: shutdown is a watch::Receiver<bool>
run_source(Some("name"), |source, mut shutdown| async move {
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = interval.tick() => { source.publish(msg).await?; }
        }
    }
    Ok(())
}).await?;

// Handler: called for each message
run_handler(Some("name"), &["topic"], |msg, handler| async move {
    let output = EmergentMessage::new("output")
        .with_causation_from_message(msg.id())
        .with_payload(json!({"processed": true}));
    handler.publish(output).await.map_err(|e| e.to_string())
}).await?;

// Sink: called for each message
run_sink(Some("name"), &["topic"], |msg| async move {
    println!("{}", msg.payload());
    Ok(())
}).await?;
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
| `connect_to(name, socket_path)` | Connect to engine at a specific socket path |
| `publish(message)` | Send a message (fire-and-forget) |
| `publish_all(messages)` | Publish all messages from an iterator, return count |
| `publish_stream(stream)` | Publish messages from an async stream, return count |
| `discover()` | List the engine's IPC type names and IPC-exposed actors. These are not topics or primitives: the sink's topology call or `GET /api/topology` lists those |
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
        let input: i64 = msg.payload()["value"].as_i64().unwrap_or(0);
        let output = input * 2;

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
| `connect_to(name, socket_path)` | Connect to engine at a specific socket path |
| `subscribe(types)` | Subscribe and get message stream |
| `publish(message)` | Send a message |
| `publish_all(messages)` | Publish all messages from an iterator, return count |
| `publish_stream(stream)` | Publish messages from an async stream, return count |
| `unsubscribe(types)` | Remove subscriptions |
| `discover()` | List the engine's IPC type names and IPC-exposed actors. These are not topics or primitives: the sink's topology call or `GET /api/topology` lists those |
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
| `messages(name, types)` | Connect, subscribe to `types`, return stream. Empty `types` falls back to the config's `subscribes` |
| `subscribe(types)` | Subscribe and get message stream |
| `get_my_subscriptions()` | Query configured subscriptions from engine |
| `unsubscribe(types)` | Remove subscriptions |
| `discover()` | List the engine's IPC type names and IPC-exposed actors. These are not topics or primitives: the sink's topology call or `GET /api/topology` lists those |
| `disconnect()` | Graceful disconnection |

### Convenience Method

```rust
// Subscribe to exactly these topics
let mut stream = EmergentSink::messages("console", ["timer.tick"]).await?;

while let Some(msg) = stream.next().await {
    println!("{}", msg.payload());
}
```

Pass an empty list to defer to the engine instead. The sink then queries its
configured `subscribes` list from the engine's TOML and subscribes to that:

```rust
let mut stream = EmergentSink::messages("console", Vec::<String>::new()).await?;
```

## MessageStream

The subscription stream:

```rust
let mut stream = handler.subscribe(["events"]).await?;

while let Some(msg) = stream.next().await {
    process(msg);
}
// Stream ends on graceful shutdown
```

## Streaming Publish

Publish a batch or async stream of messages. Each message is sent individually so subscribers begin consuming immediately. Both methods return the count of successfully published messages and stop on the first error.

### From a collection

```rust
let messages: Vec<EmergentMessage> = records.iter().map(|r| {
    EmergentMessage::new("record.imported")
        .with_payload(json!(r))
}).collect();

let count = source.publish_all(messages).await?;
```

### From an async stream

```rust
use tokio_stream::wrappers::ReceiverStream;

let (tx, rx) = tokio::sync::mpsc::channel(32);
tokio::spawn(async move {
    for i in 0..100 {
        let msg = EmergentMessage::new("batch.item")
            .with_payload(json!({"index": i}));
        let _ = tx.send(msg).await;
    }
});

let count = source.publish_stream(ReceiverStream::new(rx)).await?;
```

Both `publish_all` and `publish_stream` are available on `EmergentSource` and `EmergentHandler`.

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
| `SocketNotFound` | Engine socket does not exist |
| `ConnectionFailed` | Failed to connect to engine |
| `Timeout` | Operation timed out |
| `SubscriptionFailed` | Subscription request failed |
| `PublishFailed` | The broker rejected an acknowledged publish |
| `DiscoveryFailed` | Discovery request failed |
| `InvalidSubscriptionTopic` | A topic that could never match, such as `system.*.error` |
| `SerializationError` | A message could not be serialized or deserialized |

`ClientError` also has `IoError`, `IpcError`, `ProtocolError` and
`EngineError`. The SDK does not return any of them today. The first two have
`From` conversions so your own code can use `?` on those error types.

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
                    None => break,
                }
            }
        }
    }

    Ok(())
}
```

## Discovery

`discover()` asks the engine's IPC layer what it exposes. The reply describes the transport, not the workflow:

- `message_types` holds the IPC type names the engine has registered (`SystemEvent`, `EmergentMessage`). These are envelope names, not topics such as `timer.tick`.
- `primitives` holds the actors the engine exposes over IPC (`message_broker`). These are engine internals, not the sources, handlers and sinks in the config, and `kind` is always empty.

To list the primitives and the topics they publish and subscribe to, call `get_topology()` on a sink, which publishes `system.request.topology`, or read `GET /api/topology`.

```rust
let info = source.discover().await?;

println!("IPC type names:");
for msg_type in &info.message_types {
    println!("  - {}", msg_type);
}

println!("IPC-exposed actors:");
for actor in &info.primitives {
    println!("  - {}", actor.name);
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

- [TypeScript SDK](typescript.md) - JSR: [@govcraft/emergent](https://jsr.io/@govcraft/emergent)
- [Python SDK](python.md) - PyPI: [emergent-client](https://pypi.org/project/emergent-client/)
- [Go SDK](go.md) - `go get github.com/govcraft/emergent/sdks/go`
- [Sources](../primitives/sources.md) - Building data ingress
- [Handlers](../primitives/handlers.md) - Building transformations
- [Sinks](../primitives/sinks.md) - Building data egress
- [Configuration](../configuration.md) - TOML reference
