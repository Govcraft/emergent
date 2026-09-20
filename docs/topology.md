# Topology

The engine exposes the live topology of all running primitives through an HTTP API. This lets you inspect what's running, what each primitive subscribes to and publishes, and the current state of the system.

## HTTP API

The engine starts an HTTP server on `127.0.0.1:8891` by default. The port is configurable via `api_port` in the `[engine]` section of your config file. Set `api_port = 0` to disable the HTTP API entirely.

### GET /api/topology

Returns the full topology including the engine itself and all configured primitives.

**Request:**

```bash
curl http://127.0.0.1:8891/api/topology
```

**Response:**

```json
{
  "primitives": [
    {
      "name": "emergent-engine",
      "kind": "source",
      "state": "running",
      "publishes": [
        "system.started.*",
        "system.stopped.*",
        "system.error.*",
        "system.shutdown"
      ],
      "subscribes": [],
      "pid": 12345,
      "error": null
    },
    {
      "name": "timer",
      "kind": "source",
      "state": "running",
      "publishes": ["timer.tick"],
      "subscribes": [],
      "pid": 12346,
      "error": null
    },
    {
      "name": "filter",
      "kind": "handler",
      "state": "running",
      "publishes": ["timer.filtered", "filter.processed"],
      "subscribes": ["timer.tick"],
      "pid": 12347,
      "error": null
    },
    {
      "name": "console",
      "kind": "sink",
      "state": "running",
      "publishes": [],
      "subscribes": ["timer.filtered", "filter.processed"],
      "pid": 12348,
      "error": null
    }
  ]
}
```

### Response Fields

Each primitive in the response includes:

| Field | Type | Description |
|-------|------|-------------|
| `name` | `string` | Primitive's configured name |
| `kind` | `string` | `"source"`, `"handler"`, or `"sink"` |
| `state` | `string` | `"configured"`, `"starting"`, `"running"`, `"stopping"`, `"stopped"`, or `"failed"` |
| `publishes` | `string[]` | Message types this primitive emits |
| `subscribes` | `string[]` | Message types this primitive receives |
| `pid` | `number \| null` | OS process ID |
| `error` | `string \| null` | Error message if the primitive failed |

The engine itself appears as the first entry with name `"emergent-engine"`.

From 0.14.0 the rest of the list has a stable order: primitives are
sorted by kind in data-flow order (every `source`, then every `handler`, then
every `sink`) and by name, byte order, within a kind. Both transports serialize
the same sorted payload, so two reads of an unchanged topology are byte
identical and a consumer can diff them directly. On 0.10.10 and earlier the
order came from the engine's internal hash map and changed between reads
(Govcraft/emergent#67), so consumers had to sort for themselves.

From 0.14.0, `state`, `pid` and `error` are live: a running primitive
reports `"running"` with its pid, one that exited cleanly reports `"stopped"`
with a null pid, and one that exited non-zero reports `"failed"` with the exit
status in `error`. On 0.10.10 and earlier every managed primitive reported
`"configured"` with a null pid for the life of the engine
(Govcraft/emergent#40), so use the `system.started.*`, `system.stopped.*` and
`system.error.*` events for health there.

## Topology via Pub/Sub

The HTTP API is the direct query path. For event-driven topology updates, use the pub/sub message pattern instead:

1. Any primitive publishes `system.request.topology`
2. The engine answers with `system.response.topology`
3. Sinks subscribed to `system.response.topology` receive the full topology

The engine answers the request itself, the same way it answers
`system.request.subscriptions`. No topology-query handler is needed, and the
answer is built from the same data the HTTP API serves, so the two paths always
agree.

The response copies the request's `correlation_id` onto the message envelope.
That is where the SDKs match it, which is how `get_topology()` knows which
response is its own.

### Using the SDK

Sinks have a `get_topology()` method that does the request and the matching for
you:

```rust
let topology = sink.get_topology().await?;
for p in &topology.primitives {
    println!("{} ({}) {}", p.name, p.kind, p.state);
}
```

### Example Configuration

```toml
# Source that triggers topology queries (e.g., from an HTTP endpoint)
[[sources]]
name = "topology-api"
path = "/usr/bin/deno"
args = ["run", "--allow-env", "--allow-read", "--allow-net", "./examples/sources/topology-api/main.ts"]
enabled = true
publishes = ["system.request.topology"]

# Sink that displays topology (e.g., a web UI)
[[sinks]]
name = "topology-viewer"
path = "/usr/bin/deno"
args = ["run", "--allow-env", "--allow-read", "--allow-net", "--allow-write", "./examples/sinks/topology-viewer/main.ts"]
enabled = true
subscribes = ["system.started.*", "system.stopped.*", "system.error.*", "system.response.topology"]
```

### Message Types

| Message | Direction | Payload |
|---------|-----------|---------|
| `system.request.topology` | Any primitive publishes | `{}`; the engine matches on the envelope's `correlation_id` |
| `system.response.topology` | Engine publishes | `{"primitives": [...]}` (same format as HTTP API) |

## Use Cases

- **Monitoring dashboards**: Poll the HTTP endpoint to display live pipeline state
- **Health checks**: Verify all expected primitives are running
- **Debugging**: Inspect subscriptions and publications to trace message routing
- **Dynamic UIs**: Build topology viewers that react to pub/sub updates

## See Also

- [Core Concepts](concepts.md) - System events and message routing
- [Configuration](configuration.md) - Declaring primitives and subscriptions
