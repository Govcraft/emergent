# emergent-client

Python SDK for building event-driven workflows on the
[Emergent](https://github.com/govcraft/emergent) engine. Connect to a running
engine over Unix IPC and publish or consume messages through three typed
primitives: **Source**, **Handler**, and **Sink**.

```python
from emergent import EmergentSink

async for msg in EmergentSink.messages("my_sink", ["timer.tick"]):
    print(msg.payload)
```

## Install

```bash
pip install emergent-client
```

Or with [uv](https://docs.astral.sh/uv/):

```bash
uv add emergent-client
```

Then import:

```python
from emergent import EmergentSource, EmergentHandler, EmergentSink
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
`EmergentSink.messages` is a convenience method that connects, subscribes, and
yields messages in a single call:

```python
from emergent import EmergentSink

async for msg in EmergentSink.messages("my_sink", ["timer.tick"]):
    data = msg.payload_as(dict)
    print(f"Tick #{data['sequence']}")
```

For explicit lifecycle control, connect and subscribe separately:

```python
async with await EmergentSink.connect("my_sink") as sink:
    async with await sink.subscribe(["timer.tick", "timer.filtered"]) as stream:
        async for msg in stream:
            print(msg.message_type, msg.payload)
```

### Source -- publish messages

A Source publishes messages into the engine. It cannot subscribe:

```python
from emergent import EmergentSource

async with await EmergentSource.connect("my_source") as source:
    await source.publish("sensor.reading", {"value": 42.5, "unit": "celsius"})
```

### Handler -- subscribe and publish

A Handler subscribes to incoming messages and publishes new ones. Use
`caused_by` to link output messages to the input that triggered them:

```python
from emergent import EmergentHandler, create_message

async with await EmergentHandler.connect("order_processor") as handler:
    async with await handler.subscribe(["order.created"]) as stream:
        async for msg in stream:
            await handler.publish(
                create_message("order.processed")
                .caused_by(msg.id)
                .payload({"status": "ok"})
            )
```

## Publishing Messages

Every primitive that can publish supports three calling styles. All three
produce the same result:

```python
# Shorthand -- type string and payload dict
await source.publish("timer.tick", {"count": 1})

# MessageBuilder -- fluent API with auto-build
await source.publish(
    create_message("timer.tick").payload({"count": 1})
)

# Full EmergentMessage object
await source.publish(message)
```

## Streaming Publish

Publish a collection or async stream of messages. Each message is sent
individually so subscribers begin consuming immediately. Both methods return
the count of successfully published messages and stop on the first error.

```python
# From a list or any Iterable
messages = [
    create_message("record.imported").payload(record)
    for record in records
]
count = await source.publish_all(messages)

# From an async generator or any AsyncIterable
async def generate_messages():
    for i in range(100):
        yield create_message("batch.item").payload({"index": i})

count = await source.publish_stream(generate_messages())
```

Both `publish_all` and `publish_stream` are available on `EmergentSource` and
`EmergentHandler`.

## Building Messages

`create_message` returns a fluent builder for constructing immutable
`EmergentMessage` instances:

```python
from emergent import create_message

msg = (
    create_message("sensor.reading")
    .payload({"value": 42.5, "unit": "celsius"})
    .metadata({"sensor_id": "temp-01", "location": "room-a"})
    .build()
)
```

Link messages into traceable chains with `caused_by` and `correlated_with`:

```python
reply = (
    create_message("order.confirmed")
    .caused_by(original_msg.id)
    .correlated_with(request_id)
    .payload({"confirmed": True})
    .build()
)
```

The builder sets `id` (TypeID format) and `timestamp_ms` automatically. Call
`.build()` explicitly when you need the message object, or pass the builder
directly to `publish()`, which calls `.build()` for you.

## Subscribing to Messages

`subscribe` accepts a list or variadic arguments:

```python
# List form
stream = await sink.subscribe(["timer.tick", "timer.filtered"])

# Variadic form
stream = await sink.subscribe("timer.tick", "timer.filtered")
```

Iterate over the returned `MessageStream` with `async for`:

```python
async for msg in stream:
    data = msg.payload_as(dict)
    print(data["count"])
```

`MessageStream` implements `AsyncIterator` and the async context manager
protocol, so you can use `async with` for automatic cleanup:

```python
async with await sink.subscribe(["timer.tick"]) as stream:
    async for msg in stream:
        print(msg.payload)
```

The stream also ends when the connection to the engine is lost, so the loop
stops and the code after it runs. See [Error Handling](#error-handling).

### Typed payloads with Pydantic

`payload_as` validates dict payloads against Pydantic models automatically:

```python
from pydantic import BaseModel

class SensorReading(BaseModel):
    value: float
    unit: str

async for msg in EmergentSink.messages("my_sink", ["sensor.reading"]):
    reading = msg.payload_as(SensorReading)
    print(f"{reading.value} {reading.unit}")
```

## Resource Cleanup

All primitives implement the async context manager protocol. Use `async with`
for automatic cleanup (recommended), or call `close()` / `disconnect()`
manually:

```python
# Automatic cleanup (recommended)
async with await EmergentSink.connect("my_sink") as sink:
    ...

# Manual cleanup
sink = await EmergentSink.connect("my_sink")
# ... use sink ...
await sink.disconnect()
```

The SDK subscribes to `system.shutdown` internally. When the Emergent engine
signals a graceful shutdown, active message streams close automatically.

## Helper Functions

`run_source`, `run_handler`, and `run_sink` eliminate connection and
signal-handling boilerplate. Each helper connects, sets up SIGTERM/SIGINT
handlers, runs your callback, and disconnects on completion:

```python
import asyncio
from emergent import run_source, run_handler, run_sink, create_message

# Source -- custom event loop with shutdown signal
async def timer_logic(source, shutdown_event):
    count = 0
    while not shutdown_event.is_set():
        try:
            await asyncio.wait_for(shutdown_event.wait(), timeout=3.0)
            break
        except asyncio.TimeoutError:
            count += 1
            await source.publish(
                create_message("timer.tick").payload({"count": count})
            )

await run_source("my_timer", timer_logic)

# Handler -- called once per message
async def process(msg, handler):
    await handler.publish(
        create_message("processed").caused_by(msg.id).payload({"done": True})
    )

await run_handler("my_handler", ["raw.event"], process)

# Sink -- called once per message
async def consume(msg):
    print(msg.payload)

await run_sink("my_sink", ["timer.tick"], consume)
```

The name argument is optional. When omitted or set to `None`, the helper reads
from the `EMERGENT_NAME` environment variable.

## Error Handling

All SDK errors extend `EmergentError` and include a machine-readable `code`
property. Catch specific error types for precise control:

```python
from emergent import (
    EmergentSource,
    SocketNotFoundError,
    ConnectionError,
    TimeoutError,
)

try:
    source = await EmergentSource.connect("my_source")
except SocketNotFoundError as e:
    print(f"Engine not running at: {e.socket_path}")
except TimeoutError as e:
    print(f"Timed out after {e.timeout}s")
except ConnectionError as e:
    print(f"Connection failed: {e}")
```

### Error Types

| Error                 | Code                  | Extra Fields     | Raised when                                                                  |
| --------------------- | --------------------- | ---------------- | ---------------------------------------------------------------------------- |
| `ConnectionError`     | `CONNECTION_FAILED`   |                  | The socket cannot be reached, a request cannot be sent, or the client closes |
| `SocketNotFoundError` | `SOCKET_NOT_FOUND`    | `socket_path`    | No socket file exists at the path                                            |
| `TimeoutError`        | `TIMEOUT`             | `timeout`        | The engine does not answer a request in time                                 |
| `ProtocolError`       | `PROTOCOL_ERROR`      |                  | A frame cannot be encoded or decoded                                         |
| `SubscriptionError`   | `SUBSCRIPTION_FAILED` | `message_types`  | The engine rejects a subscription, including the one a topology query makes  |
| `PublishError`        | `PUBLISH_FAILED`      | `message_type`   | The broker rejects an acknowledged publish, or the socket refuses a publish  |
| `DiscoveryError`      | `DISCOVERY_FAILED`    |                  | The engine rejects `discover()`, or its reply is malformed                   |
| `DisposedError`       | `DISPOSED`            |                  | A closed client is used                                                      |
| `StreamError`         | `STREAM_ERROR`        |                  | `stream_offer` or `stream_consume` times out or loses its stream             |
| `ValidationError`     | `VALIDATION_ERROR`    | `field`          | A message or a subscription topic is not valid                               |

`SubscriptionError` and `DiscoveryError` extend `ConnectionError`. On SDK
release 0.13.1 and earlier they were exported and never raised: each of those failures raised a
plain `ConnectionError` with the code `CONNECTION_FAILED`. An
`except ConnectionError` still catches them, so put the specific class first.
Code that compares `code` against `CONNECTION_FAILED` for one of these
failures now sees the code in the table. `PublishError` extends
`EmergentError` directly, as it always has.

When the socket refuses a write, which is what happens once the engine has gone
away, the SDK error keeps the operating system's error as `__cause__`. After
SDK release 0.13.1 a refused `publish()` or `publish_ack()` raises
`PublishError`, and a refused `subscribe()`, `discover()` or other request
raises `ConnectionError`. On 0.13.1 and earlier all of them raised the built-in
error itself (`ConnectionResetError`, `BrokenPipeError`), which is not an
`EmergentError`, so an `except EmergentError` never saw it. The built-in
`ConnectionError` and the SDK's share a name and nothing else: code that caught
`OSError` for these calls should now catch the SDK class and read `__cause__`.

When the engine closes the connection, every call still waiting for an answer
(`publish_ack()`, `discover()`, `subscribe()`, `get_topology()`,
`get_my_subscriptions()`) raises `ConnectionError("Connection closed")` at once,
and the message stream ends, so an `async for` over it stops. That holds after
SDK release 0.13.1. On 0.13.1 and earlier each call waited out its own timeout,
30 seconds by default, and then raised `TimeoutError`. The stream ended only
when the read failed: when the engine closed the connection cleanly it never
ended, so an `async for` consumer waited until the process was stopped.

A frame the engine sends with a malformed body is never raised to the caller.
After SDK release 0.13.1 it is logged and skipped, and the subscription stays
open. On 0.13.1 and earlier one such frame ended the read loop and closed the
stream.

A message whose envelope carries a field this SDK does not know is delivered
with that field ignored, after SDK release 0.13.1. On 0.13.1 and earlier the
wire model forbade unknown fields, so one new envelope field from the engine
would have stopped every Python subscriber from receiving messages.

## Message Shape

Every message flowing through Emergent follows the same envelope:

| Field            | Type                 | Description                          |
| ---------------- | -------------------- | ------------------------------------ |
| `id`             | `str`                | Unique TypeID (`msg_<uuidv7>`)       |
| `message_type`   | `str`                | Routing key (e.g., `"timer.tick"`)   |
| `source`         | `str`                | Name of the publishing primitive     |
| `correlation_id` | `str \| None`        | Links related messages               |
| `causation_id`   | `str \| None`        | ID of the triggering message         |
| `timestamp_ms`   | `int`                | Creation time (Unix ms)              |
| `payload`        | `Any`                | User-defined data                    |
| `metadata`       | `dict[str, Any] \| None` | Optional tracing/debug data     |

Use `msg.payload_as(MyModel)` to validate and convert the payload to a typed
Pydantic model or any other type.

## System Events

The Emergent engine broadcasts lifecycle events that your primitives can
subscribe to:

| Event Pattern              | Payload Type           | Fired When                    |
| -------------------------- | ---------------------- | ----------------------------- |
| `system.started.<name>`    | `SystemEventPayload`   | Primitive started             |
| `system.stopped.<name>`    | `SystemEventPayload`   | Primitive stopped             |
| `system.error.<name>`      | `SystemEventPayload`   | Primitive failed              |
| `system.shutdown`          | `SystemShutdownPayload`| Engine shutting down          |

Use the typed payload classes for safe access:

```python
from emergent import SystemEventPayload

if msg.message_type.startswith("system.started."):
    event = msg.payload_as(SystemEventPayload)
    print(f"{event.name} ({event.kind}) started with PID {event.pid}")

if msg.message_type.startswith("system.error."):
    event = msg.payload_as(SystemEventPayload)
    if event.is_error():
        print(f"{event.name} failed: {event.error}")
```

## Requirements

- Python 3.12 or later
- A running Emergent engine with the `EMERGENT_SOCKET` environment variable set

## License

MIT OR Apache-2.0
