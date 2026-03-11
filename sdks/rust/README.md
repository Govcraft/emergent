# emergent-client

Rust SDK for building event-driven workflows on the
[Emergent](https://github.com/govcraft/emergent) engine. Connect to a running
engine over Unix IPC and publish or consume messages through three async
primitives: **Source**, **Handler**, and **Sink**.

```rust
use emergent_client::prelude::*;

#[tokio::main]
async fn main() -> emergent_client::Result<()> {
    let sink = EmergentSink::connect("my_sink").await?;
    let mut stream = sink.subscribe(["timer.tick"]).await?;

    while let Some(msg) = stream.next().await {
        println!("{:?}", msg.payload());
    }
    Ok(())
}
```

## Install

Add the crate to your project:

```bash
cargo add emergent-client
```

Then import the prelude for the most common types:

```rust
use emergent_client::prelude::*;
```

Or import individual types as needed:

```rust
use emergent_client::{EmergentSource, EmergentHandler, EmergentSink, EmergentMessage};
```

## Three Primitives

Every Emergent workflow is composed of Sources, Handlers, and Sinks. Each
primitive has a single, well-defined role:

| Primitive     | Subscribe | Publish | Role                                        |
| ------------- | --------- | ------- | ------------------------------------------- |
| **Source**    | --        | Yes     | Ingress -- bring data into the system       |
| **Handler**   | Yes       | Yes     | Processing -- transform, enrich, or route   |
| **Sink**      | Yes       | --      | Egress -- persist, display, or forward data |

## Quick Start

### Sink -- consume messages

A Sink subscribes to message types and processes each one as it arrives.
`EmergentSink::messages` is a convenience method that connects, subscribes, and
yields messages in a single call:

```rust
use emergent_client::prelude::*;

let mut stream = EmergentSink::messages("my_sink", ["timer.tick"]).await?;

while let Some(msg) = stream.next().await {
    let data: serde_json::Value = msg.payload_as()?;
    println!("Tick: {data}");
}
```

For explicit lifecycle control, connect and subscribe separately:

```rust
let sink = EmergentSink::connect("my_sink").await?;
let mut stream = sink.subscribe(["timer.tick", "timer.filtered"]).await?;

while let Some(msg) = stream.next().await {
    println!("{} {:?}", msg.message_type(), msg.payload());
}
```

### Source -- publish messages

A Source publishes messages into the engine. It cannot subscribe:

```rust
use emergent_client::{EmergentSource, EmergentMessage};
use serde_json::json;

let source = EmergentSource::connect("my_source").await?;
source.publish(
    EmergentMessage::new("sensor.reading")
        .with_payload(json!({"value": 42.5, "unit": "celsius"}))
).await?;
```

### Handler -- subscribe and publish

A Handler subscribes to incoming messages and publishes new ones. Use
`with_causation_from_message` to link output messages to the input that
triggered them:

```rust
use emergent_client::prelude::*;
use serde_json::json;

let handler = EmergentHandler::connect("order_processor").await?;
let mut stream = handler.subscribe(["order.created"]).await?;

while let Some(msg) = stream.next().await {
    handler.publish(
        EmergentMessage::new("order.processed")
            .with_causation_from_message(msg.id())
            .with_payload(json!({"status": "ok"}))
    ).await?;
}
```

## Publishing Messages

Every primitive that can publish supports two calling styles. Both produce
the same result:

```rust
use emergent_client::{EmergentMessage, create_message};
use serde_json::json;

// Builder pattern with fluent API
source.publish(
    EmergentMessage::new("timer.tick")
        .with_payload(json!({"count": 1}))
).await?;

// Factory function (matches Python and TypeScript SDKs)
source.publish(
    create_message("timer.tick")
        .with_payload(json!({"count": 1}))
).await?;
```

## Building Messages

`EmergentMessage::new` and `create_message` return a builder with fluent
methods for constructing messages:

```rust
use emergent_client::{EmergentMessage, create_message};
use serde_json::json;

let msg = create_message("sensor.reading")
    .with_payload(json!({"value": 42.5, "unit": "celsius"}))
    .with_metadata(json!({"sensor_id": "temp-01", "location": "room-a"}));
```

Link messages into traceable chains with `with_causation_from_message` and
`with_correlation_id`:

```rust
use emergent_client::types::CorrelationId;

let reply = EmergentMessage::new("order.confirmed")
    .with_causation_from_message(original_msg.id())
    .with_correlation_id(CorrelationId::new())
    .with_payload(json!({"confirmed": true}));
```

The builder sets `id` (TypeID format `msg_<uuidv7>`) and `timestamp_ms`
automatically.

## Subscribing to Messages

`subscribe` accepts any type that implements `IntoSubscription` -- a single
`&str`, an array, a slice, or a `Vec`:

```rust
// Single topic
let stream = sink.subscribe("timer.tick").await?;

// Array of topics
let stream = sink.subscribe(["timer.tick", "timer.filtered"]).await?;

// Slice of topics
let stream = sink.subscribe(&["timer.tick", "timer.filtered"]).await?;

// From a Vec
let topics = vec!["timer.tick".to_string()];
let stream = sink.subscribe(topics).await?;
```

Iterate over the returned `MessageStream` with `while let`:

```rust
while let Some(msg) = stream.next().await {
    let data: MyPayload = msg.payload_as()?;
    println!("{data:?}");
}
```

`MessageStream` implements `futures::Stream`, so you can use `StreamExt`
combinators (re-exported in the prelude):

```rust
use emergent_client::prelude::*;

stream
    .filter(|msg| futures::future::ready(
        msg.message_type().as_str().starts_with("timer.")
    ))
    .for_each(|msg| async move {
        println!("{:?}", msg);
    })
    .await;
```

### Typed payloads with serde

`payload_as` deserializes the JSON payload into any type that implements
`serde::DeserializeOwned`:

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SensorReading {
    value: f64,
    unit: String,
}

while let Some(msg) = stream.next().await {
    let reading: SensorReading = msg.payload_as()?;
    println!("{} {}", reading.value, reading.unit);
}
```

## Resource Cleanup

Call `disconnect()` to cleanly close the connection. The SDK sends an
unsubscribe-all message so the server sees a normal EOF rather than a
connection reset:

```rust
let source = EmergentSource::connect("my_source").await?;
// ... use source ...
source.disconnect().await?;
```

The SDK subscribes to `system.shutdown` internally. When the Emergent engine
signals a graceful shutdown, active message streams close automatically.

## Helper Functions

`run_source`, `run_handler`, and `run_sink` eliminate connection, signal
handling, and shutdown boilerplate. Each helper connects, sets up SIGTERM
handlers, runs your async closure, and disconnects on completion.

Import them from the `helpers` module:

```rust
use emergent_client::helpers::{run_source, run_handler, run_sink};
```

### Source -- custom event loop with shutdown signal

```rust
use emergent_client::helpers::run_source;
use emergent_client::EmergentMessage;
use serde_json::json;
use std::time::Duration;

run_source(Some("my_timer"), |source, mut shutdown| async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    let mut count = 0u64;

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = interval.tick() => {
                count += 1;
                let msg = EmergentMessage::new("timer.tick")
                    .with_payload(json!({"count": count}));
                source.publish(msg).await.map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}).await?;
```

### Handler -- called once per message

```rust
use emergent_client::helpers::run_handler;
use emergent_client::EmergentMessage;
use serde_json::json;

run_handler(
    Some("my_handler"),
    &["timer.tick"],
    |msg, handler| async move {
        let output = EmergentMessage::new("timer.processed")
            .with_causation_from_message(msg.id())
            .with_payload(json!({"processed": true}));
        handler.publish(output).await.map_err(|e| e.to_string())
    }
).await?;
```

### Sink -- called once per message

```rust
use emergent_client::helpers::run_sink;

run_sink(
    Some("my_sink"),
    &["timer.tick"],
    |msg| async move {
        println!("Received: {:?}", msg.payload());
        Ok(())
    }
).await?;
```

The name argument is optional. When set to `None`, the helper reads from the
`EMERGENT_NAME` environment variable, falling back to a default.

## Error Handling

All SDK operations return `emergent_client::Result<T>`, which uses
`ClientError` as the error type. Match on specific variants for precise
control:

```rust
use emergent_client::{EmergentSource, ClientError};

match EmergentSource::connect("my_source").await {
    Ok(source) => { /* connected */ }
    Err(ClientError::SocketNotFound(path)) => {
        eprintln!("Engine not running at: {path}");
    }
    Err(ClientError::Timeout) => {
        eprintln!("Connection timed out");
    }
    Err(ClientError::ConnectionFailed(reason)) => {
        eprintln!("Connection failed: {reason}");
    }
    Err(e) => {
        eprintln!("Unexpected error: {e}");
    }
}
```

### Error Variants

| Variant               | Description                                   |
| --------------------- | --------------------------------------------- |
| `ConnectionFailed`    | Engine connection failed                      |
| `SocketNotFound`      | Engine socket does not exist at expected path  |
| `Timeout`             | Operation timed out                           |
| `ProtocolError`       | Unexpected message from engine                |
| `SubscriptionFailed`  | Subscription request rejected                 |
| `PublishFailed`       | Publish request failed                        |
| `DiscoveryFailed`     | Discovery request failed                      |
| `SerializationError`  | Message serialization/deserialization error    |
| `IoError`             | Underlying I/O error                          |
| `IpcError`            | Low-level IPC protocol error                  |
| `EngineError`         | Engine returned an application-level error     |

Helper functions use a separate `HelperError` type with variants for
connection, subscription, signal setup, and user-function errors.

## Message Shape

Every message flowing through Emergent follows the same envelope:

| Field            | Type                          | Description                            |
| ---------------- | ----------------------------- | -------------------------------------- |
| `id`             | `MessageId`                   | Unique TypeID (`msg_<uuidv7>`)         |
| `message_type`   | `MessageType`                 | Routing key (e.g., `"timer.tick"`)     |
| `source`         | `PrimitiveName`               | Name of the publishing primitive       |
| `correlation_id` | `Option<CorrelationId>`       | Links related messages                 |
| `causation_id`   | `Option<CausationId>`         | ID of the triggering message           |
| `timestamp_ms`   | `Timestamp`                   | Creation time (Unix ms)                |
| `payload`        | `serde_json::Value`           | User-defined data                      |
| `metadata`       | `Option<serde_json::Value>`   | Optional tracing/debug data            |

All identifier types (`MessageId`, `CorrelationId`, `CausationId`) use the
TypeID format and are available from `emergent_client::types`.

Use `msg.payload_as::<T>()` to deserialize the payload into any
`serde::DeserializeOwned` type.

## System Events

The Emergent engine broadcasts lifecycle events that your primitives can
subscribe to:

| Event Pattern              | Payload Type           | Fired When                    |
| -------------------------- | ---------------------- | ----------------------------- |
| `system.started.<name>`    | `SystemEventPayload`   | Primitive started             |
| `system.stopped.<name>`    | `SystemEventPayload`   | Primitive stopped             |
| `system.error.<name>`      | `SystemEventPayload`   | Primitive failed              |
| `system.shutdown`          | `SystemShutdownPayload`| Engine shutting down          |

Use the typed payload structs for safe access:

```rust
use emergent_client::types::{SystemEventPayload, SystemShutdownPayload};

if msg.message_type().as_str().starts_with("system.started.") {
    let event: SystemEventPayload = msg.payload_as()?;
    println!("{} ({}) started", event.name(), event.kind());
}

if msg.message_type().as_str().starts_with("system.error.") {
    let event: SystemEventPayload = msg.payload_as()?;
    if let Some(error) = event.error() {
        eprintln!("{} failed: {error}", event.name());
    }
}
```

## Requirements

- Rust 2024 edition (1.85+)
- Tokio async runtime
- A running Emergent engine with the `EMERGENT_SOCKET` environment variable set
- Unix platform (Linux or macOS) -- the SDK communicates over Unix domain sockets

## License

MIT OR Apache-2.0
