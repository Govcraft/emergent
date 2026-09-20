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

`shutdown` fires on SIGTERM, which is how the engine asks for a graceful stop.
After engine 0.10.10 it also fires when the engine's connection reaches EOF, so
a Source built on `run_source` stops instead of publishing into a dead socket
when the engine is SIGKILLed or aborts (Govcraft/emergent#56). Select on it
rather than looping on the publish result. A Source that drives its own loop on
a bare `EmergentSource` gets no such signal and has to exit on a failed publish
itself.

The Python, TypeScript and Go helpers do the same after SDK release 0.13.1:
`run_source` sets its `shutdown_event`, `runSource` aborts its `AbortSignal` and
`RunSource` cancels its `context.Context` when the engine closes the connection,
whether the read ends in EOF or in a reset. On 0.13.1 and earlier those three
reacted to SIGTERM and SIGINT only, so a Source that caught its publish errors
kept running against a dead socket (Govcraft/emergent#83). An engine-spawned
Source on Linux was covered anyway, because the engine asks the kernel to signal
its children when it dies. A Source started by hand, or on macOS, was not.

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

The closure takes the handler by value. It is a clone that shares the one IPC
connection, so publishing from it is the same connection the helper subscribed
on.

On emergent-client 0.13.1 and earlier the bound was
`F: Fn(EmergentMessage, &EmergentHandler) -> Fut`, which rejected any closure
holding `handler` across an `.await`: the example above failed to compile with
"lifetime may not live long enough" (Govcraft/emergent#41). A handler that
publishes is nearly every handler, so on those versions write the loop below
instead. After 0.13.1 either form works.

#### A handler that publishes: the loop

Prefer the loop when the handler keeps state between messages. It owns the
state outright, so it needs no `Arc` or `Mutex`.

```rust
use emergent_client::{EmergentHandler, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut handler = EmergentHandler::connect("my_handler").await?;
    let mut stream = handler.subscribe(["timer.tick"]).await?;

    // Ends when the engine sends system.shutdown or the connection closes.
    while let Some(msg) = stream.next().await {
        let output = EmergentMessage::new("timer.processed")
            .with_causation_from_message(msg.id())
            .with_payload(json!({"processed": true}));
        handler.publish(output).await?;
    }

    handler.disconnect().await?;
    Ok(())
}
```

`connect` takes the name as `&str`. The helpers resolve `None` from
`EMERGENT_NAME`; with the loop, read it yourself:
`std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "my_handler".into())`.

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
| `connect_to` | `async fn connect_to(name: &str, socket_path: &Path) -> Result<Self>` | Connect to an explicit socket instead of `EMERGENT_SOCKET` |
| `publish` | `async fn publish(&self, message: EmergentMessage) -> Result<()>` | Publish, fire-and-forget. `Ok` means queued, not delivered: see Publish guarantees below |
| `publish_ack` | `async fn publish_ack(&self, message: EmergentMessage) -> Result<()>` | Publish and wait for the engine's acknowledgment |
| `publish_stats` | `fn publish_stats(&self) -> PublishStats` | Counts of accepted, rejected and unanswered `publish` calls. After engine 0.13.1 |
| `publish_all` | `async fn publish_all(&self, messages: impl IntoIterator<Item = EmergentMessage>) -> Result<usize>` | Publish each message with `publish_ack`; returns the count |
| `publish_stream` | `async fn publish_stream<S>(&self, stream: S) -> Result<usize>` | Same, from an async `Stream` |
| `discover` | `async fn discover(&self) -> Result<DiscoveryInfo>` | List the engine's IPC type names and IPC-exposed actors. These are not topics or primitives: the sink's topology call or `GET /api/topology` lists those |
| `name` | `fn name(&self) -> &str` | Get the source name |
| `disconnect` | `async fn disconnect(&self) -> Result<()>` | Gracefully disconnect |

#### EmergentHandler

```rust
let mut handler = EmergentHandler::connect("my_handler").await?;  // subscribe takes &mut self
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
| `connect_to` | `async fn connect_to(name: &str, socket_path: &Path) -> Result<Self>` | Connect to an explicit socket |
| `messages` | `async fn messages(name, types) -> Result<(Self, MessageStream)>` | Connect and subscribe to `types`. Pass an empty list to use the **config's** `subscribes` instead. Clients up to 0.13.1 ignored `types` and always used the config |
| `subscribe` | `async fn subscribe(&mut self, types: impl IntoSubscription) -> Result<MessageStream>` | Subscribe and get stream |
| `publish` | `async fn publish(&self, message: EmergentMessage) -> Result<()>` | Publish, fire-and-forget. `Ok` means queued, not delivered: see Publish guarantees below |
| `publish_ack` | `async fn publish_ack(&self, message: EmergentMessage) -> Result<()>` | Publish and wait for the engine's acknowledgment |
| `publish_stats` | `fn publish_stats(&self) -> PublishStats` | Counts of accepted, rejected and unanswered `publish` calls. After engine 0.13.1 |
| `publish_all` / `publish_stream` | as on `EmergentSource` | Acked batch publish; returns the count |
| `stream_offer` / `stream_consume` | see Pull-Based Streaming below | Consumer-driven streaming |
| `discover` | `async fn discover(&self) -> Result<DiscoveryInfo>` | List the engine's IPC type names and IPC-exposed actors. These are not topics or primitives: the sink's topology call or `GET /api/topology` lists those |
| `get_my_subscriptions` | `async fn get_my_subscriptions(&self) -> Result<Vec<String>>` | The `subscribes` list from the engine config |
| `name` | `fn name(&self) -> &str` | Get handler name |
| `subscribed_types` | `fn subscribed_types(&self) -> &[String]` | Types passed to the last `subscribe` call |
| `disconnect` | `async fn disconnect(&self) -> Result<()>` | Gracefully disconnect |

The Rust SDK has no `unsubscribe` (Python, TypeScript, and Go do). To stop
receiving, call `stream.close()` or drop the stream.

#### EmergentSink

```rust
let mut sink = EmergentSink::connect("my_sink").await?;  // subscribe takes &mut self
let topics = sink.get_my_subscriptions().await?;
let mut stream = sink.subscribe(&topics).await?;
while let Some(msg) = stream.next().await {
    println!("Received: {:?}", msg.payload());
}

// Or the convenience method, which does exactly the three lines above.
// Its second argument is IGNORED: the stream carries the config's `subscribes`.
let mut stream = EmergentSink::messages("my_sink", ["timer.tick"]).await?;
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `connect` | `async fn connect(name: &str) -> Result<Self>` | Connect as sink |
| `connect_to` | `async fn connect_to(name: &str, socket_path: &Path) -> Result<Self>` | Connect to an explicit socket |
| `messages` | `async fn messages(name, types) -> Result<MessageStream>` | Connect and subscribe to `types`. Pass an empty list to use the **config's** `subscribes` instead. Clients up to 0.13.1 ignored `types` and always used the config |
| `subscribe` | `async fn subscribe(&mut self, types: impl IntoSubscription) -> Result<MessageStream>` | Subscribe and get stream |
| `discover` | `async fn discover(&self) -> Result<DiscoveryInfo>` | List the engine's IPC type names and IPC-exposed actors. These are not topics or primitives: the sink's topology call or `GET /api/topology` lists those |
| `get_my_subscriptions` | `async fn get_my_subscriptions(&self) -> Result<Vec<String>>` | Get configured subscriptions |
| `get_topology` | `async fn get_topology(&self) -> Result<TopologyState>` | Publishes `system.request.topology` and waits up to 30 s for `system.response.topology`. Engines after 0.10.10 answer it at once. On 0.10.10 and earlier nothing answers, so it returns `ClientError::Timeout` (Govcraft/emergent#46); use `GET /api/topology` there |
| `name` | `fn name(&self) -> &str` | Get sink name |
| `subscribed_types` | `fn subscribed_types(&self) -> &[String]` | Types passed to the last `subscribe` call |
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

// With correlation ID (request-response). The argument is a CorrelationId,
// not a string: use emergent_client::types::CorrelationId, then
// CorrelationId::new() to mint one or CorrelationId::parse(s)? to adopt one.
let msg = EmergentMessage::new("api.request")
    .with_correlation_id(correlation_id)
    .with_payload(request_data);

// Carry an inbound message's correlation forward (None is a no-op)
let msg = EmergentMessage::new("api.response")
    .with_correlation_id_option(input.correlation_id.as_ref())
    .with_causation_from_message(input.id());

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
// plain string otherwise). Usually unnecessary: set unwrap_stdout = true
// in the primitive's config and the SDK does this automatically.
let msg = msg.unwrap_stdout();
```

### IntoSubscription Trait

Every form takes either an exact message type or a terminal-wildcard prefix.
After engine 0.10.10, `"timer.*"` delivers every `timer.` type and `"*"`
delivers everything; on 0.10.10 and earlier both were accepted and delivered
nothing. The star has to be last: `"tim*.tick"` is rejected with
`Error::InvalidSubscriptionTopic` rather than accepted and starved.

```rust
// `handler` must be a `let mut` binding for all of these
handler.subscribe("timer.tick").await?;                        // Single string
handler.subscribe("timer.*").await?;                           // Terminal wildcard
handler.subscribe(["timer.tick", "timer.filtered"]).await?;    // Array
handler.subscribe(&["timer.tick"]).await?;                     // Slice
handler.subscribe(vec!["timer.tick".to_string()]).await?;      // Vec<String>
```

Overlapping topics deliver one copy: subscribing to both `"timer.tick"` and
`"timer.*"` yields a single `timer.tick` message, not two.

**One stream per client, in all four SDKs.** Pass every topic to one `subscribe`
call. What a second call on the same Handler or Sink does differs:

- **Python, TypeScript, Go:** the second call ends the earlier stream, so an
  `async for`, `for await` or channel range over it stops, and returns a new
  stream. The engine keeps the earlier subscriptions, so the new stream receives
  the earlier topics as well as the new ones. The earlier stream ends when the
  second call starts, even if that call then fails. That holds after SDK release
  0.13.1. On 0.13.1 and earlier the earlier stream was left open and unfed, and
  nothing ever ended it, not connection loss, not `system.shutdown`, not
  `close()`, so its consumer loop waited for ever (Govcraft/emergent#86).
- **Rust:** the second call returns `SubscriptionFailed`, because a client hands
  its push channel to its first stream. After emergent-client 0.13.1 the refusal
  comes before anything is sent, so the first stream and `subscribed_types()`
  stay as they were. On 0.13.1 and earlier the engine was asked first and the
  call failed afterwards, so the refused topics started arriving on the first
  stream while `subscribed_types()` did not list them.

To read two sets of topics apart from each other, connect two clients.

### MessageStream

```rust
let mut stream = handler.subscribe(&["timer.tick"]).await?;
while let Some(msg) = stream.next().await {
    // Process message
}
// Stream ends on: system.shutdown, connection close, or stream.close()
```

### Streaming Publish (Batch)

Available on `EmergentSource` and `EmergentHandler` in all four SDKs.

```rust
// Publish every message from an iterator; returns count published.
// Each one goes through publish_ack, so this waits for the engine per message
// and stops at the first error.
let count = source.publish_all(messages).await?;

// Publish from an async stream (e.g., a channel); returns count published
use tokio_stream::wrappers::ReceiverStream;
let count = source.publish_stream(ReceiverStream::new(rx)).await?;
```

Naming per SDK: Rust/Python `publish_all` / `publish_stream`, TypeScript `publishAll` / `publishStream`, Go `PublishAll` / `PublishStream(ctx, ch)`.

### Publish guarantees

`publish` and `publish_ack` promise different things, and the difference
matters as soon as a primitive emits more than a trickle.

| Call | Success means | A rejection shows up as |
|------|---------------|-------------------------|
| `publish` | The message was queued for the engine, in publish order | A `WARN` log line and a `publish_stats().rejected` increment |
| `publish_ack` | The broker stored the event and handed it to every subscriber's queue | An error to the caller, carrying the engine's error text |

Every IPC connection is rate limited to **100 messages per second with a burst
of 50** (acton-reactive 9.3.0 defaults). Each enabled primitive holds exactly
one connection, so those are per-primitive budgets. Over the budget the engine
refuses the message and it is never delivered. A full broker mailbox
(`TARGET_BUSY`) and a shutting-down engine (`SHUTTING_DOWN`) refuse it the same
way.

After engine 0.13.1 the Rust SDK claims the engine's answer to every `publish`,
logs a refusal at `WARN` with the engine's error text and the message type, and
counts it in `publish_stats()`. On 0.13.1 and earlier it dropped that answer at
`trace` level and `publish` returned success over lost messages. The Go, Python
and TypeScript SDKs own their read loops and, also after engine 0.13.1, log an
unmatched ERROR frame at error level instead of discarding it.

A primitive that has to sustain more than 100 messages per second should use
`publish_ack` (which cannot outpace the broker), batch several records into one
message, or raise acton's own limit in
`$XDG_CONFIG_HOME/acton/ipc.toml` under `[rate_limit]`. That limit is not an
`emergent.toml` key.

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
    helpers::{run_source, run_sink},
};
use emergent_client::types::{CorrelationId, MessageId, MessageType};
use serde_json::json;

// Or everything at once: the three clients, EmergentMessage, create_message,
// MessageStream, IntoSubscription, ClientError, Result, the system event
// payload types, and futures::StreamExt.
use emergent_client::prelude::*;
```

`create_message("domain.event")` is a free-function alias for
`EmergentMessage::new`. Other builder and accessor methods not shown above:
`with_source(&str)`, `with_causation_id(id)`, `source() -> &PrimitiveName`, and
`has_stdout_payload() -> bool`.

### Environment variables the SDKs read

The engine sets all of these for a managed primitive. They matter when you run
a primitive by hand or wonder where its logs went.

| Variable | Read by | Effect |
|---|---|---|
| `EMERGENT_SOCKET` | all four SDKs | Engine socket path. Rust falls back to the XDG default path when it is unset; Python, TypeScript, and Go fail to connect with an error naming the variable |
| `EMERGENT_NAME` | the `run_*` helpers in all four SDKs | The primitive's name when the helper is given none (`None`, `undefined`, `""`). The low-level `connect(name)` does not read it |
| `EMERGENT_LOG` | all four SDKs | `stderr` sends SDK logs to stderr. Otherwise they go to `~/.local/share/emergent/<name>/primitive.log`, which is why a managed primitive looks silent. Any other value is a log level (Rust takes a full tracing filter such as `emergent_client=trace`); TypeScript and Go also accept `off` |
| `EMERGENT_UNWRAP_STDOUT` | all four SDKs | `true` replaces an exec-source `{command, stdout, exit_code}` payload with its parsed `stdout` before your code sees it. The engine sets it to `true` only when the config has `unwrap_stdout = true`. By hand: all four SDKs trim the value, ignore letter case, take `true` or `1`, and treat anything else, including `false`, as off. On SDK release 0.13.1 and earlier the SDKs disagreed: Rust and Go needed exactly `true` or `1` (so `TRUE` was off), TypeScript needed exactly `true`, and Python treated any non-empty value (including `false`) as on (Govcraft/emergent#75) |

**Signals differ by SDK.** The Rust `run_*` helpers trap SIGTERM only, so
Ctrl-C on a hand-run Rust primitive kills it without the graceful disconnect.
Python, TypeScript, and Go trap both SIGTERM and SIGINT. Under the engine this
does not matter: shutdown arrives as `system.shutdown` and then SIGTERM. A
Source's shutdown signal also fires when the engine closes the connection, in
all four SDKs (see `run_source` above).

### Errors an engine rejection raises

When the engine rejects a request, each SDK reports it under a name that says
which request failed.

| Rejected request | Rust `ClientError` | Python | TypeScript | Go |
|---|---|---|---|---|
| subscribe, pattern subscribe | `SubscriptionFailed` | `SubscriptionError` | `SubscriptionError` | `*SubscriptionError` |
| acknowledged publish | `PublishFailed` | `PublishError` | `PublishError` | `*PublishError` |
| `discover` | `DiscoveryFailed` | `DiscoveryError` | `DiscoveryError` | `*DiscoveryError` |

On SDK release 0.13.1 and earlier TypeScript exported all three classes and
threw none of them, and Python raised only `PublishError`: every other
rejection was a plain `ConnectionError` with the code `CONNECTION_FAILED`
(Govcraft/emergent#68). After 0.13.1 the TypeScript three and Python's
`SubscriptionError` and `DiscoveryError` are raised, and each is a subclass of
`ConnectionError`, so code that catches `ConnectionError` keeps working. Test
for the specific class first. Python's `PublishError` is not a
`ConnectionError`.

In Go the same three types also wrap a failure that is not a rejection: a
timeout, a lost connection, a cancelled context. After SDK release 0.13.1 they
carry it in an `Err` field with `Unwrap`, so `errors.As(err, &timeoutErr)` and
`errors.Is(err, context.Canceled)` work, and `Err` is nil for a rejection. On
0.13.1 and earlier the cause was flattened into `Msg` and could not be reached
(Govcraft/emergent#73). `*ConnectionError` has the same `Err` and `Unwrap`, set
for a refused dial and a failed write and nil for "not connected" and
"connection closed", so `errors.Is(err, syscall.ECONNREFUSED)` works
(Govcraft/emergent#78).

A socket that refuses a write is not a rejection either. After SDK release
0.13.1 a failed TypeScript `publish()` throws a `PublishError` whose `cause` is
the `Deno.errors.*` error, where 0.13.1 and earlier threw that error itself
(Govcraft/emergent#75). Python follows the same contract after 0.13.1: a refused
`publish()` or `publish_ack()` raises `PublishError`, a refused `subscribe()`,
`discover()` or other request raises `ConnectionError`, and the built-in
`ConnectionResetError` or `BrokenPipeError` is its `__cause__`. On 0.13.1 and
earlier Python raised the built-in error itself, which no `except
EmergentError` catches (Govcraft/emergent#77). Go reports it as a
`*PublishError` wrapping the `net` error.

When the engine closes the connection, every call still waiting for an answer
fails at once with a `ConnectionError` ("connection closed"), and the message
stream ends, so a `for await`, `async for` or channel range over it stops. That
holds for Go, Python and TypeScript after SDK release 0.13.1. On 0.13.1 and
earlier an acknowledged publish, `discover`, subscribe, topology or
subscriptions call in flight waited out its own timeout, 30 seconds by default,
and reported a timeout (Govcraft/emergent#62 for Go, Govcraft/emergent#80 for
Python and TypeScript). On those releases the TypeScript stream never ended and
the Python stream ended only when the read failed, not on a clean close, so a
consumer loop could wait for ever on a dead engine. In Rust the wait ends with
`ConnectionFailed` as soon as acton's client sees the connection go.

Rust's `ClientError` also has `IoError`, `IpcError`, `ProtocolError` and
`EngineError`. The SDK returns none of them. The first two have `From`
conversions for your own `?`.

### A malformed frame from the engine

After SDK release 0.13.1 the Python, TypeScript and Go read loops log and skip
a frame whose body does not decode or has the wrong shape, and deliver the
frames behind it. On 0.13.1 and earlier one such frame (a PUSH with a null
body, a wrong-typed field, a truncated MessagePack or JSON body) ended the
TypeScript and Python read loops and closed the subscriber stream
(Govcraft/emergent#64). Go kept running but dropped every frame buffered
behind the bad one, and passed a message with wrong-typed fields to the
subscriber with an empty `ID`. A header that cannot be trusted (an oversized
length, a wrong protocol version) still drops the buffer, because nothing says
where the next frame starts.

An envelope field an SDK does not know is not malformed. All four SDKs ignore
it and deliver the message. Python is the late one: on SDK release 0.13.1 and
earlier its wire model forbade unknown fields, so a message with one was
refused (Govcraft/emergent#74).

### A socket that takes only part of a frame

After SDK release 0.13.1 the TypeScript SDK writes every byte of a frame and
writes frames one at a time. On 0.13.1 and earlier it called `Deno.Conn.write`
once and ignored the byte count, so a full socket buffer (a large payload, a
slow engine) could leave half a frame on the wire, after which the engine could
not find the next frame on that connection (Govcraft/emergent#69). Python, Go
and Rust never had the gap: asyncio's transport keeps what the socket does not
take and sends it in order, Go's `net.Conn.Write` writes everything or returns
an error, and Rust hands every frame to acton-reactive's one writer task, which
uses `write_all`.

### Cargo.toml

```toml
[dependencies]
emergent-client = "0.14"  # Use latest version from crates.io
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
import { runHandler, createMessage } from "jsr:@govcraft/emergent";

await runHandler("my_handler", ["data.raw"], async (msg, handler) => {
  const data = msg.payloadAs<Record<string, unknown>>();
  const output = createMessage("data.processed")
    .causedBy(msg.id)
    .payload({ ...data, processed: true });
  await handler.publish(output);
});
```

### runSource

```typescript
import { runSource, createMessage } from "jsr:@govcraft/emergent";

await runSource("my_source", async (source, shutdown) => {
  let count = 0;
  const timer = setInterval(async () => {
    count++;
    await source.publish(createMessage("timer.tick").payload({ count }));
  }, 3000);

  // `shutdown` is an AbortSignal, not a promise. `await shutdown` resolves at
  // once and the source exits on startup, so wait for the abort event.
  await new Promise<void>((resolve) => {
    if (shutdown.aborted) return resolve();
    shutdown.addEventListener("abort", () => resolve(), { once: true });
  });
  clearInterval(timer);
});
```

Messages are built with `createMessage(type)`, which returns a builder with
`.causedBy(id)`, `.payload(obj)`, and friends. `EmergentMessage` is the received
type and has no `new`.

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
    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunHandler("my_handler", []string{"sensor.reading"}, func(msg *emergent.EmergentMessage, handler *emergent.EmergentHandler) error {
        var data map[string]any
        if err := msg.PayloadAs(&data); err != nil {
            return err
        }
        output, err := emergent.NewMessage("sensor.processed")
        if err != nil {
            return err
        }
        output.WithCausationFromMessage(msg.ID)
        output.WithPayload(map[string]any{"processed": data, "handler": "go"})
        return handler.Publish(output)
    })
}
```

### RunSink

```go
package main

import (
    "fmt"
    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunSink("my_sink", []string{"sensor.processed"}, func(msg *emergent.EmergentMessage) error {
        fmt.Printf("Received: %v\n", msg.Payload)
        return nil
    })
}
```

Only `RunSource`'s callback takes a `context.Context`. The handler and sink
callbacks do not, and an unused `"context"` import is a compile error in Go. On
a message, `ID` and `Payload` are struct fields, and `PayloadAs` fills a pointer
and returns an `error`.
