# Emergent SDK API Reference

SDKs are available for **Rust**, **Python**, **TypeScript/Deno**, and **Go**.

## Rust SDK (`emergent-client` crate)

### Helper Functions (Recommended)

The simplest way to build custom primitives. Handle connection, signals, and shutdown automatically.

#### run_source

```rust
use emergent_client::helpers::run_source;
use emergent_client::EmergentMessage;
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}
```

#### run_handler

```rust
use emergent_client::helpers::run_handler;
use emergent_client::EmergentMessage;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}
```

#### run_sink

```rust
use emergent_client::helpers::run_sink;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_sink(
        Some("my_sink"),
        &["timer.processed"],
        |msg| async move {
            println!("Received: {:?}", msg.payload());
            Ok(())
        }
    ).await?;
    Ok(())
}
```

### Client Types (Low-Level)

For more control than the helper functions provide.

#### EmergentSource

```rust
let source = EmergentSource::connect("my_source").await?;
source.publish(message).await?;
source.disconnect().await?;
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `connect` | `async fn connect(name: &str) -> Result<Self>` | Connect to engine as a source |
| `publish` | `async fn publish(&self, message: EmergentMessage) -> Result<()>` | Publish a message |
| `discover` | `async fn discover(&self) -> Result<DiscoveryInfo>` | Discover available message types |
| `name` | `fn name(&self) -> &str` | Get the source name |
| `disconnect` | `async fn disconnect(&self) -> Result<()>` | Gracefully disconnect |

#### EmergentHandler

```rust
let handler = EmergentHandler::connect("my_handler").await?;
let mut stream = handler.subscribe(&["timer.tick"]).await?;
while let Some(msg) = stream.next().await {
    let output = EmergentMessage::new("timer.processed")
        .with_causation_from_message(msg.id())
        .with_payload(transformed_data);
    handler.publish(output).await?;
}
handler.disconnect().await?;
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `connect` | `async fn connect(name: &str) -> Result<Self>` | Connect as handler |
| `subscribe` | `async fn subscribe(&self, types: impl IntoSubscription) -> Result<MessageStream>` | Subscribe and get stream |
| `unsubscribe` | `async fn unsubscribe(&self, types: &[&str]) -> Result<()>` | Unsubscribe from types |
| `publish` | `async fn publish(&self, message: EmergentMessage) -> Result<()>` | Publish a message |
| `discover` | `async fn discover(&self) -> Result<DiscoveryInfo>` | Discover message types |
| `name` | `fn name(&self) -> &str` | Get handler name |
| `subscribed_types` | `async fn subscribed_types(&self) -> Vec<String>` | Get current subscriptions |
| `disconnect` | `async fn disconnect(&self) -> Result<()>` | Gracefully disconnect |

#### EmergentSink

```rust
let sink = EmergentSink::connect("my_sink").await?;
let topics = sink.get_my_subscriptions().await?;
let mut stream = sink.subscribe(&topics).await?;
while let Some(msg) = stream.next().await {
    println!("Received: {:?}", msg.payload());
}

// Or convenience method:
let mut stream = EmergentSink::messages("my_sink", ["timer.tick"]).await?;
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `connect` | `async fn connect(name: &str) -> Result<Self>` | Connect as sink |
| `messages` | `async fn messages(name, types) -> Result<MessageStream>` | Connect + subscribe in one call |
| `subscribe` | `async fn subscribe(&self, types: impl IntoSubscription) -> Result<MessageStream>` | Subscribe and get stream |
| `get_my_subscriptions` | `async fn get_my_subscriptions(&self) -> Result<Vec<String>>` | Get configured subscriptions |
| `name` | `fn name(&self) -> &str` | Get sink name |
| `disconnect` | `async fn disconnect(&self) -> Result<()>` | Gracefully disconnect |

### EmergentMessage

```rust
use emergent_client::EmergentMessage;
use serde_json::json;

// Basic message
let msg = EmergentMessage::new("domain.event")
    .with_payload(json!({"key": "value"}));

// With causation chain (essential for handlers)
let output = EmergentMessage::new("domain.processed")
    .with_causation_from_message(input.id())
    .with_payload(processed_data);

// With correlation ID (request-response)
let msg = EmergentMessage::new("api.request")
    .with_correlation_id(correlation_id)
    .with_payload(request_data);

// With metadata
let msg = EmergentMessage::new("audit.event")
    .with_payload(event_data)
    .with_metadata(json!({"trace_id": "abc123"}));

// Access fields
let id: &MessageId = msg.id();
let msg_type: &MessageType = msg.message_type();
let payload: &serde_json::Value = msg.payload();
let typed: MyPayload = msg.payload_as()?;

// Serialization
let bytes = msg.to_json()?;
let msg = EmergentMessage::from_json(&bytes)?;
let bytes = msg.to_msgpack()?;
let msg = EmergentMessage::from_msgpack(&bytes)?;

// Unwrap exec-source's {command, stdout, exit_code} envelope:
// replaces the payload with the parsed .stdout content (JSON if parseable,
// plain string otherwise). Usually unnecessary — set unwrap_stdout = true
// in the primitive's config and the SDK does this automatically.
let msg = msg.unwrap_stdout();
```

### IntoSubscription Trait

```rust
handler.subscribe("timer.tick").await?;                        // Single string
handler.subscribe(["timer.tick", "timer.filtered"]).await?;    // Array
handler.subscribe(&["timer.tick"]).await?;                     // Slice
handler.subscribe(vec!["timer.tick".to_string()]).await?;      // Vec<String>
```

### MessageStream

```rust
let mut stream = handler.subscribe(&["timer.tick"]).await?;
while let Some(msg) = stream.next().await {
    // Process message
}
// Stream ends on: system.shutdown, connection close, or explicit unsubscribe
```

### Streaming Publish (Batch)

Available on `EmergentSource` and `EmergentHandler` in all four SDKs.

```rust
// Publish every message from an iterator; returns count published
let count = source.publish_all(messages).await?;

// Publish from an async stream (e.g., a channel); returns count published
use tokio_stream::wrappers::ReceiverStream;
let count = source.publish_stream(ReceiverStream::new(rx)).await?;
```

Naming per SDK: Rust/Python `publish_all` / `publish_stream`, TypeScript `publishAll` / `publishStream`, Go `PublishAll` / `PublishStream(ctx, ch)`.

### Pull-Based Streaming (Consumer-Driven Backpressure)

Handlers can stream a collection item-by-item, advancing only when the consumer asks for the next item. Protocol: producer publishes `stream.ready` (with a `stream_id`), consumer sends `stream.pull` requests, producer emits one item per pull, then `stream.end` when exhausted.

```rust
// Producer side: serve items one at a time as pulls arrive
let mut pull_stream = handler.subscribe("stream.pull").await?;
let sent = handler.stream_offer(
    "work.item",                       // message type for each item
    items,                             // IntoIterator<Item = serde_json::Value>
    &mut pull_stream,
    std::time::Duration::from_secs(30),
).await?;

// Consumer side: pulls automatically after each item is consumed
let mut source_stream = handler.subscribe(["stream.ready", "work.item", "stream.end"]).await?;
let received = handler.stream_consume(
    "work.item",
    &mut source_stream,
    std::time::Duration::from_secs(30),
    |msg| { /* process item */ },
).await?;
```

Naming per SDK: Rust/Python `stream_offer` / `stream_consume`, TypeScript `streamOffer` / `streamConsume`, Go `StreamOffer` / `StreamConsume`. For a zero-code equivalent, use the `stream-runner` marketplace primitive (ack-based flow control).

### Common Imports

```rust
use emergent_client::{
    EmergentSource, EmergentHandler, EmergentSink, EmergentMessage,
    helpers::{run_source, run_handler, run_sink},
};
use serde_json::json;
```

### Cargo.toml

```toml
[dependencies]
emergent-client = "0.13"  # Use latest version from crates.io
tokio = { version = "1", features = ["full", "signal"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## Python SDK (`emergent-client` PyPI package)

Run with: `path = "uv"`, `args = ["run", "--with", "emergent-client", "handler.py"]`

### run_handler

```python
from emergent import run_handler, create_message

async def process(msg, handler):
    data = msg.payload_as(dict)
    enriched = {**data, "processed_by": "python"}
    await handler.publish(
        create_message("data.enriched").caused_by(msg.id).payload(enriched)
    )

import asyncio
asyncio.run(run_handler("enricher", ["data.raw"], process))
```

### run_source

```python
from emergent import run_source, create_message
import asyncio

async def emit(source, shutdown):
    count = 0
    while not shutdown.is_set():
        count += 1
        await source.publish(
            create_message("sensor.reading").payload({"count": count})
        )
        await asyncio.sleep(3)

asyncio.run(run_source("my_source", emit))
```

### run_sink

```python
from emergent import run_sink

async def handle(msg):
    data = msg.payload_as(dict)
    print(f"Received: {data}")

import asyncio
asyncio.run(run_sink("my_sink", ["data.enriched"], handle))
```

## TypeScript/Deno SDK (`jsr:@govcraft/emergent`)

### runSink

```typescript
import { runSink } from "jsr:@govcraft/emergent";

await runSink("my_sink", ["sensor.reading"], async (msg) => {
  const data = msg.payloadAs<{ temperature: number }>();
  console.log(`Temperature: ${data.temperature}`);
});
```

### runHandler

```typescript
import { runHandler, EmergentMessage } from "jsr:@govcraft/emergent";

await runHandler("my_handler", ["data.raw"], async (msg, handler) => {
  const data = msg.payloadAs<Record<string, unknown>>();
  const output = EmergentMessage.new("data.processed")
    .causedBy(msg.id)
    .payload({ ...data, processed: true });
  await handler.publish(output);
});
```

### runSource

```typescript
import { runSource, EmergentMessage } from "jsr:@govcraft/emergent";

await runSource("my_source", async (source, shutdown) => {
  let count = 0;
  const timer = setInterval(async () => {
    count++;
    const msg = EmergentMessage.new("timer.tick").payload({ count });
    await source.publish(msg);
  }, 3000);
  await shutdown;
  clearInterval(timer);
});
```

## Go SDK (`github.com/govcraft/emergent/sdks/go`)

### RunSource

```go
package main

import (
    "context"
    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunSource("my_source", func(ctx context.Context, source *emergent.EmergentSource) error {
        msg, _ := emergent.NewMessage("sensor.reading")
        msg.WithPayload(map[string]any{"temperature": 72.5})
        return source.Publish(msg)
    })
}
```

### RunHandler

```go
package main

import (
    "context"
    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunHandler("my_handler", []string{"sensor.reading"}, func(ctx context.Context, msg *emergent.EmergentMessage, handler *emergent.EmergentHandler) error {
        data := msg.PayloadAs(map[string]any{})
        output, _ := emergent.NewMessage("sensor.processed")
        output.CausedBy(msg.ID())
        output.WithPayload(map[string]any{"processed": data, "handler": "go"})
        return handler.Publish(output)
    })
}
```

### RunSink

```go
package main

import (
    "context"
    "fmt"
    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunSink("my_sink", []string{"sensor.processed"}, func(ctx context.Context, msg *emergent.EmergentMessage) error {
        fmt.Printf("Received: %v\n", msg.Payload())
        return nil
    })
}
```
