# Core Concepts

Emergent is a tool-agnostic orchestration layer for anything that runs from a command line. Any executable -- an LLM, a classical ML model, a Unix utility, a custom binary -- becomes a composable building block. The engine manages process lifecycle, message routing, and graceful shutdown. You declare the pipeline topology in a TOML file.

This page explains the architecture, the three-primitive model, message format, event sourcing, and lifecycle management.

---

## The Three-Primitive Model

Every component in an Emergent pipeline is one of three types:

```
                        Emergent Engine
  ┌─────────────────────────────────────────────────────────┐
  │  Process Manager  |  IPC Server  |  Event Store        │
  └─────────────────────────────────────────────────────────┘
        │                 │
        v                 v
   ┌─────────┐      ┌───────────┐      ┌────────┐
   │ Sources │ ──>  │ Handlers  │ ──>  │ Sinks  │
   └─────────┘      └───────────┘      └────────┘
    publish          sub + pub         subscribe
    only             transform         only
```

| Primitive | Subscribe | Publish | Purpose |
|-----------|-----------|---------|---------|
| **Source** | No | Yes | Ingress: bring data into the system |
| **Handler** | Yes | Yes | Transform: process, enrich, filter, route, run models |
| **Sink** | Yes | No | Egress: output, store, forward, alert |

### Why Only Three Types?

The constraint is intentional. It makes any pipeline immediately readable:

- **Sources** never depend on other primitives' output. You always know where data enters.
- **Sinks** never produce events that could trigger loops. You always know where data exits.
- **Handlers** are the only place where transformation logic lives. You always know where processing happens.

Look at any TOML config and you can trace the entire data flow: Sources publish event types, Handlers subscribe to those types and publish new ones, Sinks subscribe to the final types.

### Exec Primitives: The Universal Adapter

The exec primitives (exec-source, exec-handler, exec-sink) are the primary integration mechanism. They turn any CLI tool into a pipeline building block without writing code:

- **exec-source**: Runs a shell command on an interval (or once), emits stdout as an event
- **exec-handler**: Subscribes to events, pipes each payload through an executable's stdin, publishes stdout as a new event
- **exec-sink**: Subscribes to events, pipes each payload through an executable (fire-and-forget)

This means any tool that reads stdin and writes stdout participates in the pipeline: `jq`, `claude -p`, `curl`, `python3 predict.py`, `awk`, `wc`, a compiled Go binary, a Rust CLI -- anything.

### Custom Primitives: The Escape Hatch

When exec primitives are not enough -- persistent state across messages, custom protocols, high-performance processing -- write a custom primitive using the SDK in Rust, TypeScript, Python, or Go. The SDKs expose identical patterns. See the [SDK documentation](sdks/) for details.

---

## Message Flow

All communication uses `EmergentMessage`, a universal envelope:

```
EmergentMessage {
    id:             "msg_<UUIDv7>"       // Unique, time-sortable
    message_type:   "timer.tick"         // Subscription matching
    source:         "my_timer"           // Emitting primitive
    timestamp_ms:   1710350630000        // Unix epoch milliseconds
    payload:        { ... }              // Your data (JSON)
    metadata:       { ... }              // Optional routing/debug info
    correlation_id: "msg_..."           // Request-response grouping
    causation_id:   "msg_..."           // Event chain tracking
}
```

**Routing works by message type matching:**

1. Source publishes `timer.tick`
2. Engine matches `timer.tick` against all Handler and Sink subscriptions
3. Matching primitives receive the message via push notification
4. Handler processes, publishes `timer.filtered`
5. Engine routes `timer.filtered` to subscribed Sinks

**Subscription patterns:**

- Exact match: `"timer.tick"` matches only `timer.tick`
- Wildcard: `"system.started.*"` matches `system.started.timer`, `system.started.filter`, etc.

**Fan-out**: Multiple primitives subscribe to the same message type. One source event reaches multiple handlers or sinks simultaneously.

**Fan-in**: Multiple primitives publish the same message type. A single handler or sink receives events from many sources.

---

## Causation and Correlation

### Causation: "What caused this message?"

When a Handler transforms a message, it sets `causation_id` to link output to input:

```
timer.tick (msg_01a2)
  └─> timer.filtered (msg_01a3, caused by msg_01a2)
        └─> alert.sent (msg_01a4, caused by msg_01a3)
```

Following `causation_id` chains traces any event back to its root cause.

### Correlation: "What request does this belong to?"

For request-response patterns where multiple messages relate to one logical request:

```
http.request (msg_01a2, correlation: msg_01a2)
  ├─> db.query (msg_01a3, correlation: msg_01a2)
  └─> http.response (msg_01a4, correlation: msg_01a2)
```

Both IDs are set by the publishing primitive. The engine routes messages regardless of these IDs -- they exist for tracing and debugging.

---

## Event Sourcing

Every message is persisted to the event store before routing. The store provides:

- **JSON logs**: Append-only files (one per day) for fast writes and human readability
- **SQLite database**: Structured storage for queries, filtering, and replay

This enables:

- **Replay**: Re-process historical events through new or modified handlers
- **Debugging**: Trace any event through its entire causation chain
- **Audit**: Complete record of all system activity with timestamps

```toml
[event_store]
json_log_dir = "./logs"
sqlite_path = "./events.db"
retention_days = 30
```

---

## Lifecycle Management

### Startup Order

The engine starts primitives in a specific order to ensure consumers are ready before producers:

1. **Sinks** start first -- ready to receive
2. **Handlers** start next -- ready to process
3. **Sources** start last -- produce events only when the pipeline is ready

Within each tier, primitives start in the order they appear in the configuration file. This matters when one primitive depends on another's `system.started.*` event.

### Three-Phase Shutdown

When the engine receives SIGTERM or Ctrl+C:

1. **Stop Sources**: SIGTERM to all source processes. No new messages enter the system.
2. **Drain Handlers**: Broadcast `system.shutdown`. Handlers finish pending work, then disconnect.
3. **Drain Sinks**: Sinks consume remaining messages, flush buffers, then disconnect.

This ensures zero message loss. The SDK handles `system.shutdown` automatically -- streams close gracefully.

### Process Isolation

Each primitive runs as a separate child process. This means:

- A crashed handler does not take down the pipeline
- A slow model call does not block other handlers
- Different primitives can use different languages and runtimes
- Memory and CPU are isolated per primitive

---

## System Events

The engine publishes lifecycle events that primitives can subscribe to:

| Event | Payload | When |
|-------|---------|------|
| `system.started.<name>` | `{name, kind}` | Primitive connected |
| `system.stopped.<name>` | `{name, kind}` | Primitive disconnected |
| `system.error.<name>` | `{name, kind, error}` | Primitive failed |
| `system.shutdown.requested` | | Shutdown requested -- cleanup window |
| `system.shutdown` | `{kind}` | Graceful shutdown in progress |

System events enable reactive patterns. The [ouroboros-loop example](../config/examples/ouroboros-loop.toml) subscribes to `system.started.webhook` to bootstrap a self-seeding loop automatically.

---

## IPC Protocol

Primitives communicate with the engine over Unix domain sockets:

- **Wire format**: MessagePack (binary, efficient) or JSON (human-readable, useful for debugging)
- **Framing**: Length-prefixed messages with type header
- **Transport**: Unix domain sockets (no network overhead)

The SDK abstracts the protocol entirely. You call `source.publish()` or `handler.subscribe()` and the details are handled.

**Environment variables set by the engine for each primitive:**

| Variable | Purpose |
|----------|---------|
| `EMERGENT_SOCKET` | Path to Unix socket |
| `EMERGENT_NAME` | Primitive's configured name |
| `EMERGENT_API_PORT` | HTTP API port for topology queries |
| `EMERGENT_PUBLISHES` | Comma-separated publish types from config |
| `EMERGENT_SUBSCRIBES` | Comma-separated subscribe types from config |

---

## Topology API

The engine exposes an HTTP API for live topology queries:

```bash
curl http://127.0.0.1:8891/api/topology
```

Returns all primitives with their names, types, subscriptions, publications, PIDs, and status. Default port is 8891; configure via `api_port` in the `[engine]` section (set to `0` to disable).

For event-driven topology updates, primitives can publish `system.request.topology` and subscribe to `system.response.topology`. See the [Topology reference](topology.md).

---

## Design Philosophy

- **Tool-agnostic**: The engine does not know or care what runs inside a primitive. An LLM, a regex, a Python model, a jq filter -- all are equal.
- **Configuration-driven**: The TOML file is the source of truth for your pipeline topology. Code lives in the primitives; architecture lives in the config.
- **Single-node**: No distributed coordination complexity. One machine, one engine, one socket.
- **Process isolation**: Primitives are separate processes. Failures are contained. Languages are mixed freely.
- **Zero-code entry point**: Marketplace exec primitives handle most use cases. Custom SDK primitives are the escape hatch, not the front door.
