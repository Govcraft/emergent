# Getting Started with Emergent

*Build your first event-driven workflow*

## Abstract

Building distributed workflows requires coordinating processes that emit, transform, and consume events—typically involving message brokers, orchestrators, and complex infrastructure. Developers need a simpler path. Emergent reduces workflow engines to three primitives: Sources (publish events), Handlers (transform events), and Sinks (consume events). Unlike traditional engines requiring infrastructure setup, Emergent runs as a single binary managing components as child processes over Unix sockets. This guide shows you how to build your first pipeline, from running the example to writing your own components in Rust, TypeScript, or Python. **You can start processing events with zero infrastructure and learn patterns that scale from development to production.**

---

## 1. Introduction: Three Primitives, Zero Infrastructure

Most workflow engines force you to think about infrastructure before writing code. You install message brokers, configure databases, deploy orchestrators, then finally write your business logic. Emergent reverses this: you write async functions that publish or consume messages, then point a single binary at your configuration file. The engine handles process management, message routing, and graceful shutdown.

This approach works because Emergent constrains your options. Every component you write fits into one of three categories:

- **Source**: Publishes messages into the system (ingress from timers, webhooks, files)
- **Handler**: Receives messages, processes them, publishes new messages (transformations, enrichment)
- **Sink**: Receives messages, produces side effects (console output, HTTP calls, database writes)

These three primitives define all possible workflows. A Source cannot receive messages. A Sink cannot publish messages. A Handler does both. This constraint makes workflows easy to reason about: data flows in one direction, from Sources through Handlers to Sinks.

**You will spend this guide building one example of each primitive. By the end, you will understand the pattern well enough to build production workflows.**

---

## 2. Run the Example Pipeline

Before writing code, run the example pipeline to see Emergent in action. This pipeline demonstrates all three primitives working together.

First, clone and build the workspace:

```bash
git clone https://github.com/emergent/emergent
cd emergent
cargo build --release
```

This builds the engine (`emergent`) and three example primitives: `timer` (Source), `filter` (Handler), and `console` (Sink). The configuration at `config/emergent.toml` describes how these connect.

Start the engine:

```bash
./target/release/emergent --config ./config/emergent.toml
```

You should see output like this:

```
[17:23:45.123] [STARTED] timer (source) [msg_01a2]
[17:23:45.124] [STARTED] filter (handler) [msg_01a3]
[17:23:45.125] [STARTED] console (sink) [msg_01a4]
[17:23:50.000] [FILTER] tick #1 filtered not_multiple_of_5
[17:23:55.000] [FILTER] tick #2 filtered not_multiple_of_5
[17:24:00.000] [FILTER] tick #3 filtered not_multiple_of_5
[17:24:05.000] [FILTER] tick #4 filtered not_multiple_of_5
[17:24:10.000] [FILTER] tick #5 passed every_5th
[17:24:10.001] [FILTERED] tick #5 (every_5th every 5)
```

Here is what happened:

1. The engine started three child processes (timer, filter, console)
2. The timer Source emits a `timer.tick` event every 5 seconds
3. The filter Handler receives each tick, publishes `filter.processed` for every tick, and publishes `timer.filtered` for every 5th tick
4. The console Sink subscribes to `filter.processed` and `timer.filtered`, printing formatted output

Stop the engine with Ctrl+C. You will see graceful shutdown:

```
[17:24:15.000] [STOPPED] timer (source) [msg_01a8]
[17:24:15.100] [STOPPED] filter (handler) [msg_01a9]
[17:24:15.200] [STOPPED] console (sink) [msg_01aa]
```

The engine stopped the timer (no new events), waited for the filter to drain (process remaining events), then stopped the console (consume final output). This three-phase shutdown ensures no message loss.

**The example ran with zero configuration beyond a TOML file. You did not install a message broker or database. The engine handled process lifecycle and message routing.**

---

## 3. Understanding the Configuration

**The configuration file is your executable architecture diagram.** Everything about your workflow lives in one TOML file: which processes run, what they publish/subscribe to, how they connect. The engine enforces this contract at runtime.

Open `config/emergent.toml`. The file has five sections.

### Engine Settings

```toml
[engine]
name = "emergent"
socket_path = "auto"
wire_format = "messagepack"  # binary format for efficiency; use "json" for debugging
```

The engine creates a Unix socket at a standard location in your home directory (`~/.local/share/emergent/emergent.sock` by default). Primitives connect to this socket using the path from the `EMERGENT_SOCKET` environment variable. The wire format defaults to MessagePack (a compact binary encoding); use `"json"` when debugging message content.

### Event Store

```toml
[event_store]
json_log_dir = "./logs"
sqlite_path = "./events.db"
retention_days = 30
```

The engine writes all messages to append-only JSON logs (one file per day) and a SQLite database. You can replay workflows from these logs. The retention policy deletes events older than 30 days.

### Sources

```toml
[[sources]]
name = "timer"
path = "./target/release/timer"
args = ["--interval", "5000"]
enabled = true
publishes = ["timer.tick"]
```

Each Source declares a name, executable path, arguments, and the message types it publishes. The engine validates that Sources only publish declared types. The `enabled` flag lets you disable components without removing them.

### Handlers

```toml
[[handlers]]
name = "filter"
path = "./target/release/filter"
args = ["--filter-every", "5"]
enabled = true
subscribes = ["timer.tick"]
publishes = ["timer.filtered", "filter.processed"]
```

Handlers declare both subscriptions (input) and publications (output). The engine routes messages based on these declarations. A Handler receives only the message types it subscribes to.

### Sinks

```toml
[[sinks]]
name = "console"
path = "./target/release/console"
enabled = true
subscribes = ["timer.filtered", "filter.processed", "system.started.*", "system.stopped.*"]
```

Sinks declare only subscriptions. They cannot publish. The wildcard `system.started.*` matches all system startup events (e.g., `system.started.timer`, `system.started.filter`).

**New team members can read this file and understand your entire workflow topology—no code diving required.**

---

## 4. The Three Primitives: Mental Model

Every Emergent component follows an async event loop:

1. Connect to the engine
2. Loop: publish or receive messages
3. Disconnect on shutdown signal

The primitive type (Source, Handler, Sink) determines which operations you can perform.

### Source Mental Model

A Source runs an infinite loop that publishes messages at intervals, on external triggers (webhooks, file changes), or in response to other systems. Sources are **blind**: they publish events without knowing who receives them.

```
┌─────────────┐
│   Source    │
│             │
│  ┌───────┐  │
│  │ async │  │
│  │ loop  │──┼──> publish("timer.tick", data)
│  └───────┘  │
└─────────────┘
```

### Handler Mental Model

A Handler runs an async loop that receives messages, processes them, and publishes new messages. Handlers form the transformation pipeline.

```
┌─────────────────────────┐
│       Handler           │
│                         │
│  subscribe(["timer.tick"])
│         │               │
│         ▼               │
│  ┌────────────┐         │
│  │ async loop │         │
│  │  process   │─────────┼──> publish("timer.filtered", result)
│  └────────────┘         │
└─────────────────────────┘
```

### Sink Mental Model

A Sink runs an async loop that receives messages and produces side effects (writes to console, sends HTTP requests, appends to files). Sinks are **terminal**: they end the data flow.

```
┌─────────────────────────┐
│         Sink            │
│                         │
│  subscribe(["timer.filtered"])
│         │               │
│         ▼               │
│  ┌────────────┐         │
│  │ async loop │         │
│  │   output   │         │
│  └────────────┘         │
└─────────────────────────┘
```

**These patterns are identical across Rust, TypeScript, and Python. Once you learn the pattern in one language, you know it in all three.**

---

## 5. Write Your First Source

Create a new Rust project for a simple timer Source:

```bash
cargo new --bin my_timer
cd my_timer
cargo add emergent-client tokio serde_json clap --features tokio/full,clap/derive
```

Edit `src/main.rs`:

```rust
use emergent_client::{EmergentSource, EmergentMessage};
use serde_json::json;
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The engine sets EMERGENT_NAME when spawning this process
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_timer".to_string());

    let source = EmergentSource::connect(&name).await?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    let mut count = 0;

    loop {
        tokio::select! {
            // Handle shutdown signal from engine
            _ = sigterm.recv() => {
                source.disconnect().await?;
                break;
            }
            // Publish on each interval tick
            _ = interval.tick() => {
                count += 1;
                let message = EmergentMessage::new("my_timer.tick")
                    .with_payload(json!({"count": count}));
                source.publish(message).await?;
            }
        }
    }
    Ok(())
}
```

This Source does four things:

1. **Gets its name** from the `EMERGENT_NAME` environment variable (the engine sets this)
2. **Connects** to the engine using `EmergentSource::connect()`
3. **Runs an async loop** that publishes messages every 3 seconds
4. **Handles SIGTERM** for graceful shutdown

The `tokio::select!` macro races two async operations: receiving SIGTERM or waiting for the interval. When SIGTERM arrives (sent by the engine during shutdown), the Source disconnects and exits.

Build the Source:

```bash
cargo build --release
```

Add it to your configuration (`config/emergent.toml`):

```toml
[[sources]]
name = "my_timer"
path = "/path/to/my_timer/target/release/my_timer"
enabled = true
publishes = ["my_timer.tick"]
```

**You wrote about 25 lines of Rust. The engine handles process management, socket creation, and shutdown coordination.**

---

## 6. Write Your First Handler

Create a Handler that receives your timer ticks and doubles the count:

```bash
cargo new --bin my_doubler
cd my_doubler
cargo add emergent-client tokio serde serde_json --features tokio/full
```

Edit `src/main.rs`:

```rust
use emergent_client::{EmergentHandler, EmergentMessage};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::signal::unix::{signal, SignalKind};

#[derive(Deserialize)]
struct TickPayload {
    count: u64,
}

#[derive(Serialize)]
struct DoubledPayload {
    original: u64,
    doubled: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_doubler".to_string());

    let handler = EmergentHandler::connect(&name).await?;
    let mut stream = handler.subscribe(&["my_timer.tick"]).await?;
    let mut sigterm = signal(SignalKind::terminate())?;

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                handler.disconnect().await?;
                break;
            }
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        let tick: TickPayload = msg.payload_as()?;
                        let doubled = DoubledPayload {
                            original: tick.count,
                            doubled: tick.count * 2,
                        };

                        // Link output to input for tracing
                        let output = EmergentMessage::new("my_timer.doubled")
                            .with_causation_id(msg.id())
                            .with_payload(json!(doubled));

                        handler.publish(output).await?;
                    }
                    None => break, // Stream closed (graceful shutdown)
                }
            }
        }
    }
    Ok(())
}
```

This Handler introduces three new concepts:

1. **Subscription**: `handler.subscribe(&["my_timer.tick"])` creates a stream that yields messages
2. **Typed payloads**: `msg.payload_as::<TickPayload>()` deserializes the JSON payload into a Rust struct
3. **Causation tracking**: `.with_causation_id(msg.id())` links the output message to the input message

The causation ID creates an event chain: `my_timer.tick` (ID: `msg_01a2`) causes `my_timer.doubled` (causation ID: `msg_01a2`). This enables tracing events through your system—we'll explore this fully in Section 8.

Build and configure:

```bash
cargo build --release
```

```toml
[[handlers]]
name = "my_doubler"
path = "/path/to/my_doubler/target/release/my_doubler"
enabled = true
subscribes = ["my_timer.tick"]
publishes = ["my_timer.doubled"]
```

**Handlers transform events while preserving traceability. Every output message knows which input caused it.**

---

## 7. Write Your First Sink

Create a Sink that prints doubled values:

```bash
cargo new --bin my_printer
cd my_printer
cargo add emergent-client tokio serde --features tokio/full
```

Edit `src/main.rs`:

```rust
use emergent_client::EmergentSink;
use serde::Deserialize;
use tokio::signal::unix::{signal, SignalKind};

#[derive(Deserialize)]
struct DoubledPayload {
    original: u64,
    doubled: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("EMERGENT_NAME")
        .unwrap_or_else(|_| "my_printer".to_string());

    let sink = EmergentSink::connect(&name).await?;
    let mut stream = sink.subscribe(&["my_timer.doubled"]).await?;
    let mut sigterm = signal(SignalKind::terminate())?;

    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                sink.disconnect().await?;
                break;
            }
            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        let data: DoubledPayload = msg.payload_as()?;
                        println!("{} doubled is {}", data.original, data.doubled);
                    }
                    None => break,
                }
            }
        }
    }
    Ok(())
}
```

This Sink looks almost identical to the Handler, except it uses `EmergentSink` and never calls `publish()`. Sinks are write-only to the external world.

Build and configure:

```bash
cargo build --release
```

```toml
[[sinks]]
name = "my_printer"
path = "/path/to/my_printer/target/release/my_printer"
enabled = true
subscribes = ["my_timer.doubled"]
```

Run the engine with your three-component pipeline:

```bash
./target/release/emergent --config ./config/emergent.toml
```

You should see:

```
1 doubled is 2
2 doubled is 4
3 doubled is 6
...
```

**You built a complete event-driven pipeline with three independent programs that communicate through the engine.** Each program is simple, testable, and replaceable—want to switch from console to database logging? Just swap the Sink in your config.

---

## 8. Message Structure and Tracing

Now that you've written all three primitives, let's examine the message structure that makes their communication possible.

Every message in Emergent shares the same structure:

```rust
EmergentMessage {
    id: MessageId,              // Time-sortable unique ID (UUIDv7 format)
    message_type: String,       // "timer.tick", "timer.doubled"
    source: String,             // "my_timer", "my_doubler"
    timestamp_ms: u64,          // Unix epoch milliseconds
    payload: Value,             // Your data
    metadata: Option<Value>,    // Optional key-value pairs
    correlation_id: Option<MessageId>,  // Request-response pairing
    causation_id: Option<MessageId>,    // Event chain tracking
}
```

### Message IDs

Message IDs are time-sortable UUIDs (UUIDv7 format)—you can sort by ID to get chronological order. The engine generates IDs automatically when you call `EmergentMessage::new()`.

### Message Types

Message types use dotted notation: `domain.event`. Conventions:

- `timer.tick` - domain event from the timer
- `timer.filtered` - derived event after filtering
- `system.started.timer` - system event for timer startup
- `system.shutdown` - broadcast signal for graceful shutdown

Wildcards work in subscriptions: `subscribe(&["system.*"])` matches all system events.

### Causation vs Correlation

**Causation ID** answers "what caused this message?" Use it to trace event chains:

```
timer.tick (msg_01a2)
  ├─> timer.filtered (msg_01a3, caused by msg_01a2)
  │     └─> email.sent (msg_01a4, caused by msg_01a3)
  └─> filter.processed (msg_01a5, caused by msg_01a2)
```

**Correlation ID** answers "what request does this belong to?" Use it for request-response patterns:

```
http.request (msg_01a2, correlation: msg_01a2)
  ├─> db.query (msg_01a3, correlation: msg_01a2)
  └─> http.response (msg_01a4, correlation: msg_01a2)
```

Set causation in Handlers to track transformations:

```rust
let output = EmergentMessage::new("processed.event")
    .with_causation_id(input_msg.id())
    .with_payload(result);
```

Set correlation for request-response workflows:

```rust
let request_id = msg.id();
let response = EmergentMessage::new("http.response")
    .with_correlation_id(request_id)
    .with_payload(data);
```

**Causation chains and correlation IDs make distributed workflows traceable. You can reconstruct the entire flow from logs.**

---

## 9. Polyglot Workflows

You can mix languages in a single workflow. The SDKs for Rust, TypeScript, and Python expose identical APIs.

### TypeScript Sink Example

Create a TypeScript Sink using Deno:

```typescript
#!/usr/bin/env -S deno run --allow-env --allow-net=unix

import { EmergentSink } from "./sdks/ts/mod.ts";

const name = Deno.env.get("EMERGENT_NAME") || "ts_printer";

for await (const msg of EmergentSink.messages(name, ["my_timer.doubled"])) {
  const { original, doubled } = msg.payloadAs<{ original: number; doubled: number }>();
  console.log(`[TypeScript] ${original} → ${doubled}`);
}
```

Make the file executable and add it to your configuration:

```bash
chmod +x my_sink.ts
```

```toml
[[sinks]]
name = "ts_printer"
path = "/usr/bin/deno"
args = ["run", "--allow-env", "--allow-net=unix", "/path/to/my_sink.ts"]
enabled = true
subscribes = ["my_timer.doubled"]
```

### Python Source Example

Create a Python Source that emits HTTP webhook events:

```python
#!/usr/bin/env python3
import asyncio
import os
from aiohttp import web
from emergent import EmergentSource

source = None

async def handle_webhook(request):
    body = await request.json()
    if source:
        await source.publish("webhook.received", body)
    return web.json_response({"status": "ok"})

async def main():
    global source
    name = os.environ.get("EMERGENT_NAME", "webhook")
    source = await EmergentSource.connect(name)

    app = web.Application()
    app.router.add_post("/webhook", handle_webhook)

    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", 8080)
    await site.start()

    await asyncio.Event().wait()  # Run forever

if __name__ == "__main__":
    asyncio.run(main())
```

Add to configuration:

```toml
[[sources]]
name = "webhook"
path = "/usr/bin/python3"
args = ["/path/to/webhook.py"]
enabled = true
publishes = ["webhook.received"]
```

Now your pipeline mixes Rust (timer), Python (webhook), and TypeScript (console output). The engine handles communication for all three.

### When to Use Each Language

| Use Case | Language | Why |
|----------|----------|-----|
| Performance-critical handlers | Rust | Compile-time safety, zero-cost abstractions |
| Data transformation | Python | Pandas, NumPy ecosystem |
| Web integrations | TypeScript | Native HTTP, JSON handling |
| Quick prototypes | Python/TypeScript | Faster iteration cycles |
| Production Sources | Rust | Reliability, resource efficiency |

**Language choice becomes a per-component decision. Use Python for data science, TypeScript for web integrations, Rust for performance-critical transformations.**

---

## 10. Graceful Shutdown Explained

When you stop the engine (Ctrl+C or SIGTERM), it executes a three-phase shutdown:

### Phase 1: Stop Sources

The engine sends SIGTERM to all Source processes. Sources stop accepting new inputs and disconnect. This prevents new messages from entering the system.

### Phase 2: Drain Handlers

The engine broadcasts a `system.shutdown` message. Handlers receive this message on their subscription stream, finish processing pending messages, then disconnect. The SDK automatically closes the stream when it receives `system.shutdown`.

### Phase 3: Drain Sinks

After all Handlers disconnect, the engine waits for Sinks to consume remaining messages. Sinks receive `system.shutdown`, process any buffered messages, then disconnect.

This three-phase approach ensures:

- No messages are lost
- Handlers finish processing pending work
- Sinks flush output buffers

You only need to handle SIGTERM and check for `None` on the message stream:

```rust
loop {
    tokio::select! {
        _ = sigterm.recv() => {
            handler.disconnect().await?;
            break;
        }
        msg = stream.next() => {
            match msg {
                Some(msg) => { /* process */ }
                None => break,  // Stream closed by SDK
            }
        }
    }
}
```

The SDK handles `system.shutdown` internally. When the stream returns `None`, graceful shutdown is complete.

**Graceful shutdown requires no code beyond handling SIGTERM and checking for stream closure. The engine orchestrates the drain sequence.**

---

## 11. Next Steps

You now understand Emergent's core concepts: three primitives, message structure, configuration, and shutdown. Here are patterns to explore next.

### Multiple Subscriptions

Handlers can subscribe to multiple message types:

```rust
let mut stream = handler.subscribe(&[
    "timer.tick",
    "webhook.received",
]).await?;
```

Use pattern matching on `msg.message_type` to route messages:

```rust
match msg.message_type.as_str() {
    "timer.tick" => handle_tick(msg).await?,
    "webhook.received" => handle_webhook(msg).await?,
    _ => {}
}
```

### Fan-out and Fan-in

Multiple Sinks can subscribe to the same message type (fan-out):

```toml
[[sinks]]
name = "console"
subscribes = ["timer.tick"]

[[sinks]]
name = "logger"
subscribes = ["timer.tick"]

[[sinks]]
name = "metrics"
subscribes = ["timer.tick"]
```

Multiple Handlers can publish the same message type (fan-in):

```toml
[[handlers]]
name = "enricher_1"
publishes = ["data.enriched"]

[[handlers]]
name = "enricher_2"
publishes = ["data.enriched"]
```

A single Sink subscribes to `data.enriched` and receives messages from both Handlers.

### Error Handling

Publish error events when processing fails:

```rust
match process_message(&msg).await {
    Ok(result) => {
        handler.publish(
            EmergentMessage::new("processing.success")
                .with_causation_id(msg.id())
                .with_payload(json!(result))
        ).await?;
    }
    Err(e) => {
        handler.publish(
            EmergentMessage::new("processing.error")
                .with_causation_id(msg.id())
                .with_payload(json!({"error": e.to_string()}))
        ).await?;
    }
}
```

Create a Sink that subscribes to `*.error` to centralize error handling.

### Testing Primitives

Test primitives by separating business logic from SDK calls:

```rust
#[tokio::test]
async fn test_doubler() {
    let input = EmergentMessage::new("timer.tick")
        .with_payload(json!({"count": 5}));

    let output = process_tick(input).await.unwrap();

    assert_eq!(output.message_type, "timer.doubled");
    let payload: DoubledPayload = output.payload_as().unwrap();
    assert_eq!(payload.doubled, 10);
}
```

### Exploring the Event Store

Query the SQLite event store to analyze workflows:

```sql
SELECT message_type, COUNT(*)
FROM events
WHERE timestamp_ms > ?
GROUP BY message_type;
```

Replay events by reading JSON logs and re-publishing them through a Source.

---

## 12. Conclusion

You learned Emergent's three primitives (Source, Handler, Sink), ran an example pipeline, wrote your first components, and explored message tracing and shutdown behavior. The pattern is simple: connect, loop over messages, publish or consume, disconnect on shutdown.

This simplicity scales. Production workflows use the same three primitives you practiced here. A 50-component pipeline follows the same principles as a 3-component example.

Three key takeaways:

1. **Constraints enable reasoning**: Sources publish, Handlers transform, Sinks consume. Every component fits one pattern.
2. **The engine handles complexity**: Process management, message routing, graceful shutdown all work without your code.
3. **Polyglot is practical**: Mix languages based on component requirements, not workflow requirements.

Start building workflows by identifying Sources (where does data come from?), Handlers (what transformations are needed?), and Sinks (where does data go?). Write one component at a time, test it in isolation, then compose components in the configuration file.

**The code patterns you learned are production-ready.** You'll want to add error handling, monitoring, and deployment automation, but the core structure remains the same. Deploy these primitives by pointing the engine at your configuration and starting the binary.
