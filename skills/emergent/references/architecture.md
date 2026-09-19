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
- `timer.tick`: Timer emitted a tick
- `http.request`: HTTP request received
- `metric.cpu`: CPU metric collected
- `slack.frame`: WebSocket frame from Slack
- `life.tick`: Game of Life generation clock

## IPC Protocol

### Transport Layer

- **Socket Type**: Unix domain sockets
- **Wire Format**: MessagePack, always. `[engine].wire_format` still parses
  (`"messagepack"` or `"json"`) but nothing reads it, so omit it. On engine
  0.10.10 and earlier it was inert and unannounced; after 0.10.10 setting it
  earns a warning at startup.
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

**One connection per primitive, capped.** A primitive connects once and keeps
that connection until it exits; publishing reuses the connection's writer rather
than opening another, so ingress never competes with subscribers for a slot. The
engine's IPC listener admits at most `max_connections` at a time, a limit
acton-reactive resolves from `$XDG_CONFIG_HOME/acton/ipc.toml` or its own
default and `[engine].max_connections` overrides. A connection refused at that
ceiling gets a typed `connection_rejected` response naming the limit before the
stream is dropped. After engine 0.10.10 the engine will not start a topology
whose enabled primitives, plus 4 reserved connections, exceed the effective
limit.

**A subscription is a literal message type or a terminal-wildcard prefix.**
The broker keeps two indexes. A literal topic is looked up character for
character. A topic ending in a single `*` matches every message type that
starts with the text before the star, so `system.started.*` reaches
`system.started.ticker` and `tick.*` reaches `tick.out` and `tick.exit`. `*`
alone reaches every message the engine publishes. A connection subscribed to
both `tick.out` and `tick.*` receives one copy of `tick.out`, not two.

The star is terminal. `system.*.error` matches nothing and the engine refuses
to load a topology that configures one, rather than starting a primitive that
would sit idle.

On engine 0.10.10 and earlier there was no wildcard routing at all: a topic
holding a `*` was accepted without error and then never received anything.
Topologies written for those releases listed every type explicitly, which is
still correct.

## System Events

The engine broadcasts lifecycle events into the same pub-sub fabric as application messages. Handlers and Sinks can subscribe to them like any other event; Sources cannot subscribe to anything. Every system event has `source` set to `emergent-engine`.

The type is concrete, one per primitive, so naming each one is always correct: `subscribes = ["system.error.fetch-issues", "system.error.score-severity"]`. After 0.10.10 a sink that wants every lifecycle event can subscribe to `system.error.*` instead of listing every primitive. On 0.10.10 and earlier that subscription received nothing, so those topologies list each primitive.

### Primitive Lifecycle Events

| Event | Payload | Description |
|-------|---------|-------------|
| `system.started.<name>` | `{name, kind, pid, publishes, subscribes}` | Process spawned. Fires right after the spawn, before the child has connected to the socket |
| `system.stopped.<name>` | `{name, kind, pid, publishes, subscribes}` | Process exited with code 0, code 143, or on SIGTERM. This includes a primitive that exits on its own, not only an engine-requested stop |
| `system.error.<name>` | `{name, kind, pid, publishes, subscribes, error}` | Any other exit (`error` is `"Exited with status: N"`), or the spawn itself failed (`error` is the OS error text and there is no `pid`). After 0.10.10 also `"Restarts exhausted: N attempts within W ms"` when a restart policy gives up |
| `system.restarted.<name>` | `{name, kind, pid, publishes, subscribes, restart_attempt}` | After 0.10.10 only. The restart policy respawned the primitive. `restart_attempt` is 1-based inside the current window. It follows the `system.error.<name>` or `system.stopped.<name>` that reported the exit |

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

Every SDK subscribes to `system.shutdown` on its own and swallows it, so it is three events in the event store and none in your handler. It closes the handler/sink stream whose `kind` matches. On SDK release 0.13.1 and earlier none of them did: the engine delivers the whole message envelope as the notification payload, which puts `kind` one level deeper than any of the four SDKs looked for it, so a sink there logs an unknown or empty shutdown kind on all three broadcasts and stops only on the SIGTERM that follows (Govcraft/emergent#43 for Rust, Python and TypeScript, Govcraft/emergent#49 for Go). After 0.13.1 all four SDKs read the inner envelope, and a Rust, a Python, a TypeScript and a Go sink each closed its stream within 60 ms of the `{"kind":"sink"}` broadcast, well inside the 500 ms before SIGTERM. Make SIGTERM a clean exit anyway: it is still the fallback, and it is where a primitive on 0.13.1 or earlier ends.

### Topology Events

| Event | Description |
|-------|-------------|
| `system.request.subscriptions` | SDK subscription discovery. Payload `{name}`. Answered by the engine |
| `system.response.subscriptions` | Engine's answer: `{subscribes}` from the config, with the request's `correlation_id` |
| `system.request.topology` | Topology query via pub/sub. Answered by the engine after 0.10.10; 0.10.10 and earlier never answer it |
| `system.response.topology` | Engine's answer: `{primitives: [...]}`, built by the same function as `GET /api/topology`, with the request's `correlation_id` |

The engine answers both requests itself and does not forward them to subscribers. `system.request.subscriptions` is how a primitive learns its config's `subscribes` list at runtime: the Rust SDK's `EmergentHandler::messages`, `EmergentSink::messages` and `get_my_subscriptions` use it, while a direct `subscribe([...])` call and the `run_*` helpers subscribe to the types the code passes in. On engine 0.10.10 and earlier a topology request got no answer at all (Govcraft/emergent#46), so there use `GET /api/topology`.

## HTTP API

Axum-based server on configurable port (default: 8891, `api_port = 0` to disable). It binds `127.0.0.1` only. If the port is taken the engine logs a warning and keeps running without the API.

- `GET /api/topology` returns `{"primitives": [{name, kind, state, publishes, subscribes, pid, error}]}`

The first entry is a synthetic `emergent-engine` of kind `"source"` whose `publishes` shows `system.started.*` style strings. Those are display labels for a family of concrete types rather than types the engine ever publishes under that name, though after 0.10.10 the same string does work as a subscription selector. Disabled primitives are absent.

After 0.10.10 the entries after the engine are sorted: by kind in data-flow order (sources, then handlers, then sinks) and by name within a kind. `system.response.topology` carries the same sorted list, so two reads of an unchanged topology are identical and can be diffed directly. On 0.10.10 and earlier the order came from a hash map and changed between reads (Govcraft/emergent#67).

Use it for the graph (`name`, `kind`, `publishes`, `subscribes`) on any engine. Whether it is also good for health depends on the version. On engine 0.10.10 and earlier it is not: every managed primitive reports `state: "configured"` and `pid: null` even while it is running, because the engine serves its registration-time copy (Govcraft/emergent#40). There, read liveness from `system.started.<name>`, `system.stopped.<name>` and `system.error.<name>` in the event store. After 0.10.10 `state`, `pid` and `error` are live: a running primitive reports `running` with its pid, one that exited cleanly reports `stopped` with `pid: null`, and one that crashed reports `failed` with the exit status in `error`. `starting` and `stopping` show up around those transitions.

## Engine Lifecycle

### Startup Order

The engine starts primitives in dependency order:

1. **Sinks first**: Ready to receive messages
2. **Handlers second**: Ready to transform messages
3. **Sources last**: Begin emitting messages

Within each tier, primitives start in config order. After 0.10.10 the engine then waits for that tier before starting the next: it holds until every primitive in it that declares `subscribes` has reached the engine over IPC, so a slow-starting consumer no longer misses a source's first event. A primitive that declares no `subscribes`, every source among them, is never waited on. `[engine].startup_ready_timeout_ms` (default 5000) bounds the wait; at the deadline the engine logs a warning naming the primitives it never heard from and starts the next tier anyway, and a primitive that exits or fails during the wait releases its tier at once.

The wait is an inference, not a handshake. acton-reactive exposes no per-connection identity and no subscribe callback, so the engine reads readiness from IPC traffic carrying the primitive's name (the SDK asking for its configured `subscribes`, plus a short settle) and from a subscribed IPC connection whose kernel peer pid is the primitive's child. Either is enough. A primitive that publishes nothing, hard-codes its topics rather than deferring to the config, and runs behind a wrapper that forks (so its pid is not the one the engine spawned) is still invisible and costs its tier the deadline.

On 0.10.10 and earlier there was no wait at all: a fixed 50 ms pause after each primitive and then the next tier regardless, so any subscriber slower than that missed early messages (Govcraft/emergent#66). A source that publishes the instant it starts (an `exec-source` with no interval) is the usual way to find this out there.

A primitive whose `path` does not exist stops the engine at config load. A spawn that fails later does not: the engine emits `system.error.<name>` and carries on with the rest.

**Supervision is opt-in, and absent on 0.10.10 and earlier.** By default the engine does not restart a primitive that exits or fails to spawn. It emits `system.stopped.<name>` or `system.error.<name>` and the primitive stays down until the engine restarts. After 0.10.10 a primitive can set `restart = "on-failure"` or `"always"` (see the configuration reference): the engine respawns it with a doubling backoff, emits `system.restarted.<name>`, and gives up with a `system.error.<name>` of `"Restarts exhausted: ..."` once `restart_max_retries` restarts land inside `restart_window_ms`. It never restarts anything during shutdown.

A restart is a new process. Whatever the old one held in memory is gone: a `stream-runner` forgets its batch, a stateful SDK handler forgets its accumulator. So a restart policy suits stateless primitives, which is one more reason to keep state in events. Either way, subscribe something to `system.error.<name>` so the failure is an event someone owns, and on 0.10.10 and earlier run the engine under a process supervisor.

### Shutdown Order (Three-Phase Drain)

On SIGTERM or Ctrl+C the engine broadcasts `system.shutdown.requested`, then runs three phases back to back:

1. **Phase 1: Stop Sources**
   - Broadcast `system.shutdown` with `kind: "source"` (sources cannot subscribe, so nothing receives it)
   - Send SIGTERM to every source
   - No new messages enter the system

2. **Phase 2: Drain Handlers**
   - Broadcast `system.shutdown` with `kind: "handler"`
   - Give handlers a drain window to exit on their own
   - Send SIGTERM to every handler still running

3. **Phase 3: Drain Sinks**
   - Broadcast `system.shutdown` with `kind: "sink"`
   - Give sinks a drain window to exit on their own
   - Send SIGTERM to every sink still running

The timing depends on the engine version.

**After 0.10.10** the two windows are deadlines, not sleeps, and both are configurable under `[engine]`: `shutdown_drain_ms` (default 500) is the window before SIGTERM, `shutdown_grace_ms` (default 2000) is the window after it. A phase moves on the moment every primitive in it has exited. With SDKs that honor the `system.shutdown` broadcast (the fix for Govcraft/emergent#43), a measured three-primitive topology stopped in 128 ms. A primitive built against an older SDK ignores the broadcast and waits out the drain window, so expect about half a second per tier instead. A primitive still alive at the grace deadline gets SIGKILL, sent to its whole process group so anything it spawned dies with it, and the engine logs a warning naming it. Raise `shutdown_grace_ms` for a primitive that legitimately needs longer to flush, rather than letting it be killed mid-write.

**On 0.10.10 and earlier** the same windows are fixed sleeps (500 ms before SIGTERM, 2 s after, for every phase) and nothing waits on an actual exit: the sleeps add up to 7 s whether the topology is idle or busy, and a measured shutdown of a two-primitive topology took between 6 and 14 s end to end. That engine never sends SIGKILL, so a primitive that ignores SIGTERM outlives it as an orphan.

Design for both:

- In-flight work has the drain window after `system.shutdown` before SIGTERM arrives, then the grace window before the engine moves on or kills it. Anything longer is cut off, which is one more reason a primitive does one short act.
- A clean stop is exit code 0, exit code 143, or death by SIGTERM. Anything else is recorded as `system.error.<name>` even during shutdown, and that includes a SIGKILL at the grace deadline.

### When the Engine Dies Without Shutting Down

Nothing above runs if the engine is SIGKILLed or aborts, and a release build aborts on panic. What happens to the primitives then depends on the version.

**On 0.10.10 and earlier** they keep running. Each child leads its own process group and nothing ties its lifetime to the engine's, so a source goes on publishing into a socket with no reader until somebody kills it by hand (Govcraft/emergent#56).

**After 0.10.10** two things stop them, and the second one is what covers platforms the first does not.

1. On Linux the engine arms `PR_SET_PDEATHSIG` with SIGTERM in each child between fork and exec, so the kernel signals the primitive the moment the engine goes away, whatever killed it. The signal reaches the primitive only, not its process group, so a primitive that spawns its own children is still responsible for them. The child also re-reads its parent right after arming: if the engine died in that window the signal is never coming, so the child exits instead of being reparented into an orphan.
2. Everywhere, the primitive sees its IPC connection reach EOF. Handlers and Sinks already ended their subscription stream on that, and Sources now do too: the Rust SDK's `run_source` watches the connection alongside SIGTERM and fires the same shutdown signal the user function already selects on. A Source that drives its own loop instead of using `run_source` gets nothing from this and must exit on a failed publish.

Either way the primitive stops of its own accord, so nothing needs cleaning up before the engine is restarted.

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
