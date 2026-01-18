# Core Concepts

Emergent enforces a simple constraint: data flows in one direction through three primitive types. This makes pipelines predictable, debuggable, and easy to reason about.

## The Three-Primitive Model

Every component in an Emergent pipeline is one of three types:

```
┌─────────────────────────────────────────────────────────────────┐
│                      Emergent Engine                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
│  │  Process    │  │    IPC      │  │    Event Store          │  │
│  │  Manager    │  │   Server    │  │  (JSON logs + SQLite)   │  │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
        │                 │
        ▼                 ▼
   ┌─────────┐      ┌───────────┐      ┌────────┐
   │ Sources │ ──>  │ Handlers  │ ──>  │ Sinks  │
   └─────────┘      └───────────┘      └────────┘
    publish          sub + pub         subscribe
    only             transform         only
```

### Why Only Three Types?

The constraint is intentional:

- **Sources** can only publish. They bring data into the system (HTTP webhooks, timers, file watchers).
- **Handlers** can subscribe and publish. They transform data (filter, enrich, aggregate, route).
- **Sinks** can only subscribe. They send data out (databases, logs, notifications).

This means you can look at any primitive and immediately know its role. Sources never depend on other primitives' output. Sinks never produce events that could trigger loops. Handlers are the only place where transformation logic lives.

## Message Flow

All communication uses `EmergentMessage`:

```rust
pub struct EmergentMessage {
    pub id: String,              // msg_<UUIDv7> - unique, time-ordered
    pub message_type: String,    // e.g., "timer.tick", "user.created"
    pub source: String,          // primitive name that emitted this
    pub correlation_id: Option<String>,  // groups related messages
    pub causation_id: Option<String>,    // ID of triggering message
    pub timestamp_ms: u64,       // Unix timestamp
    pub payload: serde_json::Value,      // your data
    pub metadata: Option<serde_json::Value>,
}
```

**Message routing:**

1. Source publishes `timer.tick`
2. Engine matches message type against Handler/Sink subscriptions
3. Matching primitives receive the message via push notification
4. Handler processes and publishes `timer.filtered`
5. Engine routes `timer.filtered` to subscribed Sinks

**Subscription patterns:**

- Exact match: `"timer.tick"` matches only `timer.tick`
- Wildcard: `"system.started.*"` matches `system.started.timer`, `system.started.filter`, etc.

## Event Sourcing and Causation

Every message is persisted to the event store before routing. This enables:

- **Replay**: Re-process historical events through new handlers
- **Debugging**: Trace any event back through its origin chain
- **Audit**: Complete record of all system activity

**Causation tracking:**

```rust
// Original message from timer
let tick = EmergentMessage::new("timer.tick")
    .with_payload(json!({"sequence": 5}));
// tick.id = "msg_01234..."

// Handler creates derived message with causation link
let filtered = EmergentMessage::new("timer.filtered")
    .with_causation_id(tick.id())  // Links to parent
    .with_payload(json!({"original": 5}));
```

Following `causation_id` chains lets you trace any event back to its root cause.

## Lifecycle Management

The engine manages primitive lifecycles:

**Startup:**

1. Engine starts IPC server
2. Spawns Sources, Handlers, Sinks as child processes
3. Sets environment variables (`EMERGENT_SOCKET`, `EMERGENT_NAME`)
4. Publishes `system.started.<name>` for each primitive

**Runtime:**

- Primitives connect to engine via Unix socket
- Engine routes messages based on subscriptions
- All messages logged to event store

**Shutdown (three-phase drain):**

1. **Stop Sources**: SIGTERM to sources, no new messages enter
2. **Drain Handlers**: Broadcast `system.shutdown` (kind: handler), wait for exit
3. **Drain Sinks**: Broadcast `system.shutdown` (kind: sink), ensure all events consumed

The SDK handles `system.shutdown` automatically—streams close gracefully.

## IPC Protocol

Primitives communicate with the engine over Unix domain sockets:

- **Wire format**: MessagePack (binary, efficient) or JSON (human-readable)
- **Framing**: Length-prefixed messages with type header
- **Operations**: Subscribe, unsubscribe, publish, discover

The SDK abstracts this entirely. You call `source.publish()` or `sink.subscribe()` and the protocol details are handled.

**Environment variables set by engine:**

| Variable | Purpose |
|----------|---------|
| `EMERGENT_SOCKET` | Path to Unix socket |
| `EMERGENT_NAME` | Primitive's configured name |

## System Events

The engine publishes lifecycle events:

| Event | Payload | When |
|-------|---------|------|
| `system.started.<name>` | `{name, kind}` | Primitive connected |
| `system.stopped.<name>` | `{name, kind}` | Primitive disconnected |
| `system.error.<name>` | `{name, kind, error}` | Primitive failed |
| `system.shutdown` | `{kind}` | Graceful shutdown signal |

Subscribe to `system.started.*` to monitor all primitive startups.

## Design Philosophy

Emergent is opinionated:

- **Single-node**: No distributed coordination complexity
- **Process isolation**: Primitives are separate processes, can be any language
- **Configuration-driven**: Topology declared in TOML, not code
- **Silent primitives**: Only Sinks produce output; Sources/Handlers emit domain events only
- **Engine owns lifecycle**: Primitives don't manage their own subscriptions at startup

This makes pipelines predictable. You can read the config and understand exactly what messages flow where.
