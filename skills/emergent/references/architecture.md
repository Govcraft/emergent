# Emergent Architecture Reference

## EmergentMessage Structure

The universal message envelope for all Emergent communications:

```rust
pub struct EmergentMessage {
    /// Unique message ID (TypeID format: msg_<uuid_v7>)
    pub id: MessageId,

    /// Message type for routing (e.g., "timer.tick", "http.request")
    pub message_type: MessageType,

    /// Source primitive that published this message
    pub source: PrimitiveName,

    /// Optional correlation ID for request-response patterns
    pub correlation_id: Option<CorrelationId>,

    /// Optional causation ID (ID of message that triggered this one)
    pub causation_id: Option<CausationId>,

    /// Timestamp when message was created (Unix milliseconds)
    pub timestamp_ms: Timestamp,

    /// User-defined payload (any JSON-serializable data)
    pub payload: serde_json::Value,

    /// Optional metadata for debugging, tracing, etc.
    pub metadata: Option<serde_json::Value>,
}
```

### Message ID Format

- Format: `msg_<UUIDv7>` (TypeID specification)
- UUIDv7 provides time-ordered, globally unique identifiers

### Message Type Convention

Use `{domain}.{action}` naming:
- `timer.tick` — Timer emitted a tick
- `http.request` — HTTP request received
- `metric.cpu` — CPU metric collected
- `slack.frame` — WebSocket frame from Slack
- `life.tick` — Game of Life generation clock

## IPC Protocol

### Transport Layer

- **Socket Type**: Unix domain sockets
- **Wire Format**: MessagePack, always. `[engine].wire_format` still parses
  (`"messagepack"` or `"json"`) but nothing reads it, so omit it.
- **Frame Protocol**: Length-prefixed frames from acton-reactive

### Environment Variables

Set by the engine when spawning primitives:

| Variable | Purpose |
|----------|---------|
| `EMERGENT_SOCKET` | Path to the engine's Unix socket |
| `EMERGENT_NAME` | Name of this primitive (from config) |
| `EMERGENT_PUBLISHES` | Comma-separated publish types |
| `EMERGENT_SUBSCRIBES` | Comma-separated subscribe types |
| `EMERGENT_API_PORT` | `[engine].api_port` (`0` when the HTTP API is disabled) |
| `EMERGENT_UNWRAP_STDOUT` | `true`, set only when the primitive has `unwrap_stdout = true` |

`EMERGENT_PUBLISHES` and `EMERGENT_SUBSCRIBES` are set to an empty string when
the list is empty. The child otherwise inherits the engine's own environment.
A per-primitive `env` entry replaces the inherited value, and the engine-set
variables above are applied last, so they win over a same-named `env` key.

### Connection Flow

1. Primitive reads `EMERGENT_SOCKET` to find engine socket
2. Connects to Unix socket
3. Handlers/Sinks: optionally ask the engine for their configured `subscribes`
   list (`system.request.subscriptions`, see Topology Events)
4. Sources: Publish messages. Sources have no subscribe API.
5. Handlers/Sinks: Subscribe to message types, receive push notifications

**Subscriptions are exact-match.** The broker looks a message type up by its
literal string. There is no wildcard routing: `system.started.*` or `tick.*`
is accepted without error and then never receives anything. List every type
explicitly.

## System Events

The engine broadcasts lifecycle events into the same pub-sub fabric as application messages. Handlers and Sinks can subscribe to them like any other event; Sources cannot subscribe to anything. Every system event has `source` set to `emergent-engine`.

The type is concrete, one per primitive, and subscriptions are exact-match, so name each one: `subscribes = ["system.error.fetch-issues", "system.error.score-severity"]`. A sink that wants every lifecycle event lists every primitive.

### Primitive Lifecycle Events

| Event | Payload | Description |
|-------|---------|-------------|
| `system.started.<name>` | `{name, kind, pid, publishes, subscribes}` | Process spawned. Fires right after the spawn, before the child has connected to the socket |
| `system.stopped.<name>` | `{name, kind, pid, publishes, subscribes}` | Process exited with code 0, code 143, or on SIGTERM. This includes a primitive that exits on its own, not only an engine-requested stop |
| `system.error.<name>` | `{name, kind, pid, publishes, subscribes, error}` | Any other exit (`error` is `"Exited with status: N"`), or the spawn itself failed (`error` is the OS error text and there is no `pid`) |

**Payload fields:**
- `name: String`: primitive name
- `kind: String`: one of: `"source"`, `"handler"`, `"sink"`
- `pid: u32`: process ID. Omitted when the spawn failed
- `publishes: [String]`: message types this primitive publishes. Omitted when empty
- `subscribes: [String]`: message types this primitive subscribes to. Omitted when empty
- `error: String`: error message (only in error events)

Absent means omitted, not `null` or `[]`, so read these with a default: `jq '.publishes // []'`.

### Shutdown Events

| Event | Payload | Description |
|-------|---------|-------------|
| `system.shutdown.requested` | `{}` | Engine received shutdown signal (before drain starts) |
| `system.shutdown` | `{kind: String}` | Broadcast once per drain phase, with `kind` `"source"`, then `"handler"`, then `"sink"`. Never reaches user code |

`system.shutdown.requested` fires first and reaches user code like any other message. Use it for cleanup tasks (e.g., stopping web servers started by exec-source). The engine adds no grace period after it: the drain starts immediately, so the cleanup window is only the fixed timers under Shutdown Order.

Every SDK subscribes to `system.shutdown` on its own and swallows it, so it is three events in the event store and none in your handler. It is meant to close the handler/sink stream whose `kind` matches. Do not rely on that: the engine delivers the whole message envelope as the notification payload, which puts `kind` one level deeper than the Rust, Python and TypeScript SDKs look for it. Confirmed at runtime on engine 0.10.10: an `exec-sink` logs `shutdown_kind=unknown` on all three broadcasts and stops only on the SIGTERM that follows (Govcraft/emergent#43). The Go SDK reads the right level. Assume a primitive ends on the SIGTERM fallback, and make SIGTERM a clean exit.

### Topology Events

| Event | Description |
|-------|-------------|
| `system.request.subscriptions` | SDK subscription discovery. Payload `{name}`. Answered by the engine |
| `system.response.subscriptions` | Engine's answer: `{subscribes}` from the config, with the request's `correlation_id` |
| `system.request.topology` | Topology query via pub/sub. The engine does **not** answer it |
| `system.response.topology` | Topology response, `{primitives: [...]}` in the HTTP API's format (what topology-viewer listens for) |

`system.request.subscriptions` is the only request the engine answers itself. It is how a primitive learns its config's `subscribes` list at runtime: the Rust SDK's `EmergentHandler::messages`, `EmergentSink::messages` and `get_my_subscriptions` use it, while a direct `subscribe([...])` call and the `run_*` helpers subscribe to the types the code passes in. A topology request is routed like any other message, so a response exists only if the topology contains a handler that subscribes to `system.request.topology`, reads the HTTP API, and publishes `system.response.topology`. The engine repo has one at `examples/handlers/topology-api`; no marketplace primitive does this. Without it, use `GET /api/topology`.

## HTTP API

Axum-based server on configurable port (default: 8891, `api_port = 0` to disable). It binds `127.0.0.1` only. If the port is taken the engine logs a warning and keeps running without the API.

- `GET /api/topology` returns `{"primitives": [{name, kind, state, publishes, subscribes, pid, error}]}`

The first entry is a synthetic `emergent-engine` of kind `"source"` whose `publishes` shows `system.started.*` style strings. Those are display labels, not subscribable types. Disabled primitives are absent.

Use it for the graph (`name`, `kind`, `publishes`, `subscribes`), not for health. Verified against 0.10.10: every managed primitive reports `state: "configured"` and `pid: null` even while it is running, because the engine serves its registration-time copy. For liveness, read `system.started.<name>`, `system.stopped.<name>` and `system.error.<name>` from the event store.

## Engine Lifecycle

### Startup Order

The engine starts primitives in dependency order:

1. **Sinks first** — Ready to receive messages
2. **Handlers second** — Ready to transform messages
3. **Sources last** — Begin emitting messages

Within each tier, primitives start in config order with a fixed 50 ms pause after each one. That ordering is the whole guarantee. There is no readiness handshake, so a subscriber that takes longer than that to connect and subscribe can miss early messages. A source that publishes the instant it starts (an `exec-source` with no interval) is the usual way to find this out.

A primitive whose `path` does not exist stops the engine at config load. A spawn that fails later does not: the engine emits `system.error.<name>` and carries on with the rest.

**There is no supervision.** The engine never restarts a primitive that exits or fails to spawn. It emits `system.stopped.<name>` or `system.error.<name>` and the primitive stays down until the engine restarts. If a primitive must survive its own crashes, that is a design input: keep primitives small enough that they do not crash, subscribe something to `system.error.<name>` so the failure is an event someone owns, and run the engine under a process supervisor.

### Shutdown Order (Three-Phase Drain)

On SIGTERM or Ctrl+C the engine broadcasts `system.shutdown.requested`, then runs three phases back to back:

1. **Phase 1: Stop Sources**
   - Broadcast `system.shutdown` with `kind: "source"` (sources cannot subscribe, so nothing receives it)
   - Send SIGTERM to every source
   - Sleep 2 s
   - No new messages enter the system

2. **Phase 2: Drain Handlers**
   - Broadcast `system.shutdown` with `kind: "handler"`
   - Sleep 500 ms
   - Send SIGTERM to every handler still running
   - Sleep 2 s

3. **Phase 3: Drain Sinks**
   - Broadcast `system.shutdown` with `kind: "sink"`
   - Sleep 500 ms
   - Send SIGTERM to every sink still running
   - Sleep 2 s

The timers are fixed and none is configurable. Nothing waits on an actual exit: the sleeps add up to 7 s whether the topology is idle or busy, and a measured shutdown of a two-primitive topology took between 6 and 14 s end to end. Design for that:

- In-flight work has about 500 ms after `system.shutdown` before SIGTERM arrives, then 2 s before the engine moves on. Anything longer is cut off, which is one more reason a primitive does one short act.
- The engine never sends SIGKILL. A primitive that ignores SIGTERM outlives the engine as an orphan.
- A clean stop is exit code 0, exit code 143, or death by SIGTERM. Anything else is recorded as `system.error.<name>` even during shutdown.

## Message Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Emergent Engine                              │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────────┐  │
│  │  Process        │  │    Message      │  │    Event Store      │  │
│  │  Manager        │  │    Broker       │  │  (JSON + SQLite)    │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
        │ spawns              │ routes              │ persists
        ▼                     ▼                     ▼
   ┌─────────┐          ┌───────────┐         ┌────────┐
   │ Sources │ ──────▶  │ Handlers  │ ──────▶ │ Sinks  │
   └─────────┘ publish  └───────────┘ publish └────────┘
               ←─subscribe─┘          ←─subscribe─┘
```

## Causation Tracing

Messages form a causation chain for debugging and tracing:

```
timer.tick (id: msg_001)
    │
    └── timer.filtered (id: msg_002, causation_id: msg_001)
            │
            └── notification.sent (id: msg_003, causation_id: msg_002)
```

To maintain the chain, always set causation when publishing from a handler:

```rust
let output = EmergentMessage::new("domain.processed")
    .with_causation_from_message(input_msg.id())
    .with_payload(processed_data);
```

## Correlation IDs

Use correlation IDs for request-response patterns:

```rust
// Request side
let correlation_id = CorrelationId::new();
let request = EmergentMessage::new("api.request")
    .with_correlation_id(correlation_id.clone())
    .with_payload(request_data);

// Response side (in handler)
let response = EmergentMessage::new("api.response")
    .with_correlation_id(request.correlation_id.clone().unwrap())
    .with_causation_from_message(request.id())
    .with_payload(response_data);
```
