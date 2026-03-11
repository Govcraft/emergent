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
| `state` | `string` | `"running"`, `"stopped"`, or `"failed"` |
| `publishes` | `string[]` | Message types this primitive emits |
| `subscribes` | `string[]` | Message types this primitive receives |
| `pid` | `number \| null` | OS process ID |
| `error` | `string \| null` | Error message if the primitive failed |

The engine itself appears as the first entry with name `"emergent-engine"`.

## Topology via Pub/Sub

The HTTP API is the direct query path. For event-driven topology updates, use the pub/sub message pattern instead:

1. A Source publishes `system.request.topology`
2. A Handler receives the request, queries the HTTP API, and publishes `system.response.topology`
3. Sinks subscribed to `system.response.topology` receive the full topology

This pattern keeps the engine simple (it only serves HTTP) while letting primitives build on top of it.

### Example Configuration

```toml
# Source that triggers topology queries (e.g., from an HTTP endpoint)
[[sources]]
name = "topology-api"
path = "/usr/bin/deno"
args = ["run", "--allow-env", "--allow-read", "--allow-net", "./examples/sources/topology-api/main.ts"]
enabled = true
publishes = ["system.request.topology"]

# Handler that queries the engine and publishes the response
[[handlers]]
name = "topology-query"
path = "/usr/bin/deno"
args = ["run", "--allow-env", "--allow-read", "--allow-net", "./examples/handlers/topology-api/main.ts"]
enabled = true
subscribes = ["system.request.topology"]
publishes = ["system.response.topology"]

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
| `system.request.topology` | Source/Handler publishes | `{"correlationId": "...", "requestedBy": "..."}` |
| `system.response.topology` | Handler publishes | `{"primitives": [...]}` (same format as HTTP API) |

## Use Cases

- **Monitoring dashboards**: Poll the HTTP endpoint to display live pipeline state
- **Health checks**: Verify all expected primitives are running
- **Debugging**: Inspect subscriptions and publications to trace message routing
- **Dynamic UIs**: Build topology viewers that react to pub/sub updates

## See Also

- [Core Concepts](concepts.md) - System events and message routing
- [Configuration](configuration.md) - Declaring primitives and subscriptions
