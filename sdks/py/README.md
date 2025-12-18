# Emergent Python SDK

Python SDK for the Emergent event-driven workflow platform.

## Installation

```bash
pip install emergent-client
```

Or with uv:

```bash
uv add emergent-client
```

## Quick Start

### Sink (Subscribe Only) - 3 Lines

```python
from emergent import EmergentSink

async for msg in EmergentSink.messages("my_sink", ["timer.tick"]):
    print(msg.payload)
```

### Source (Publish Only)

```python
from emergent import EmergentSource

async with await EmergentSource.connect("my_source") as source:
    await source.publish("sensor.reading", {"value": 42.5, "unit": "celsius"})
```

### Handler (Subscribe + Publish)

```python
from emergent import EmergentHandler, create_message

async with await EmergentHandler.connect("order_processor") as handler:
    async with await handler.subscribe(["order.created"]) as stream:
        async for msg in stream:
            # Process and publish with causation tracking
            await handler.publish(
                create_message("order.processed")
                .caused_by(msg.id)
                .payload({"status": "ok"})
            )
```

## Three Primitives

| Primitive | Can Subscribe | Can Publish | Use Case |
|-----------|--------------|-------------|----------|
| **Source** | No | Yes | Ingress: HTTP endpoints, sensors, external APIs |
| **Handler** | Yes | Yes | Processing: transform, enrich, route data |
| **Sink** | Yes | No | Egress: database writers, notifications, external APIs |

## Message Building

```python
from emergent import create_message

# Simple message
msg = create_message("timer.tick").payload({"count": 1}).build()

# With causation tracking
reply = (
    create_message("order.confirmed")
    .caused_by(original_msg.id)
    .payload({"confirmed": True})
    .build()
)

# With all options
msg = (
    create_message("sensor.reading")
    .payload({"value": 42.5, "unit": "celsius"})
    .metadata({"sensor_id": "temp-01", "location": "room-a"})
    .source("sensor_service")
    .build()
)
```

## Error Handling

```python
from emergent import EmergentSource, SocketNotFoundError, ConnectionError

try:
    source = await EmergentSource.connect("my_source")
except SocketNotFoundError as e:
    print(f"Engine not running at: {e.socket_path}")
except ConnectionError as e:
    print(f"Connection failed: {e}")
```

## Requirements

- Python 3.12+
- Running Emergent engine with `EMERGENT_SOCKET` environment variable set

## License

MIT OR Apache-2.0
