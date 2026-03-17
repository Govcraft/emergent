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

| Error                 | Code                  | Extra Fields     |
| --------------------- | --------------------- | ---------------- |
| `ConnectionError`     | `CONNECTION_FAILED`   |                  |
| `SocketNotFoundError` | `SOCKET_NOT_FOUND`    | `socket_path`    |
| `TimeoutError`        | `TIMEOUT`             | `timeout`        |
| `ProtocolError`       | `PROTOCOL_ERROR`      |                  |
| `SubscriptionError`   | `SUBSCRIPTION_FAILED` | `message_types`  |
| `PublishError`        | `PUBLISH_FAILED`      | `message_type`   |
| `DiscoveryError`      | `DISCOVERY_FAILED`    |                  |
| `DisposedError`       | `DISPOSED`            |                  |
| `ValidationError`     | `VALIDATION_ERROR`    | `field`          |

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
