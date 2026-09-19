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
- **Wire Format**: MessagePack (default) or JSON
- **Frame Protocol**: Length-prefixed frames from acton-reactive

### Environment Variables

Set by the engine when spawning primitives:

| Variable | Purpose |
|----------|---------|
| `EMERGENT_SOCKET` | Path to the engine's Unix socket |
| `EMERGENT_NAME` | Name of this primitive (from config) |
| `EMERGENT_PUBLISHES` | Comma-separated publish types |
| `EMERGENT_SUBSCRIBES` | Comma-separated subscribe types |

### Connection Flow

1. Primitive reads `EMERGENT_SOCKET` to find engine socket
2. Connects to Unix socket
3. Optionally discovers available message types
4. Sources: Publish messages
5. Handlers/Sinks: Subscribe to message types, receive push notifications

## System Events

The engine broadcasts lifecycle events into the same pub-sub fabric as application messages. Any primitive can subscribe to them like any other event.

### Primitive Lifecycle Events

| Event | Payload | Description |
|-------|---------|-------------|
| `system.started.<name>` | `{name, kind, pid, publishes, subscribes}` | Primitive started successfully |
| `system.stopped.<name>` | `{name, kind, pid, publishes, subscribes}` | Primitive stopped gracefully |
| `system.error.<name>` | `{name, kind, pid, publishes, subscribes, error}` | Primitive failed |

**Payload fields:**
- `name: String` — Primitive name
- `kind: String` — One of: `"source"`, `"handler"`, `"sink"`
- `pid: u32` — Process ID
- `publishes: [String]` — Message types this primitive publishes
- `subscribes: [String]` — Message types this primitive subscribes to
- `error: String` — Error message (only in error events)

### Shutdown Events

| Event | Payload | Description |
|-------|---------|-------------|
| `system.shutdown.requested` | — | Engine received shutdown signal (before drain starts) |
| `system.shutdown` | `{kind: String}` | Engine shutting down (SDK handles internally) |

`system.shutdown.requested` fires first — use it for cleanup tasks (e.g., stopping web servers started by exec-source). `system.shutdown` is then handled internally by the SDK to gracefully close handler/sink streams.

### Topology Events

| Event | Description |
|-------|-------------|
| `system.request.subscriptions` | SDK subscription discovery |
| `system.response.subscriptions` | SDK subscription response |
| `system.request.topology` | Topology query via pub/sub |
| `system.response.topology` | Topology response (used by topology-viewer) |

## HTTP API

Axum-based server on configurable port (default: 8891, `api_port = 0` to disable).

- `GET /api/topology` — Returns all primitives with state, publishes, subscribes, PID

## Engine Lifecycle

### Startup Order

The engine starts primitives in dependency order:

1. **Sinks first** — Ready to receive messages
2. **Handlers second** — Ready to transform messages
3. **Sources last** — Begin emitting messages

This ensures no messages are lost during startup.

### Shutdown Order (Three-Phase Drain)

1. **Phase 1: Stop Sources**
   - Send SIGTERM to all sources
   - Wait for graceful exit
   - No new messages enter the system

2. **Phase 2: Drain Handlers**
   - Broadcast `system.shutdown` with `kind: "handler"`
   - Wait for handlers to finish processing
   - Handlers should complete in-flight work then exit

3. **Phase 3: Drain Sinks**
   - Broadcast `system.shutdown` with `kind: "sink"`
   - Wait for sinks to consume remaining messages
   - Sinks should finish all output then exit

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
