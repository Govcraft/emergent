# Configuration Reference

The TOML configuration file is your pipeline's architecture. It declares which processes run, what events they publish and subscribe to, and how they connect. The engine reads this file at startup and enforces the declared topology.

## File Location

```bash
emergent --config /path/to/config.toml
emergent --config ./emergent.toml
```

Use `emergent init` to generate a starter config interactively.

On engine 0.10.10 and earlier an unknown key was ignored without a word, so `retension_days` or a singular `subscribe` loaded cleanly and did nothing. From 0.14.0 an unknown key is a load error that names the key, the keys its table accepts, and the file it came from.

## Complete Example

This example shows all three primitive types: a marketplace exec-source running a shell command, a Deno-based TypeScript handler, and a Python sink. The `path` field for each primitive points to any executable -- the engine spawns these as child processes.

```toml
# ======================================================================
# Engine Settings
# ======================================================================

[engine]
name = "emergent"
socket_path = "auto"
# api_port = 8891  # HTTP API port (0 to disable)

# ======================================================================
# Event Store Settings
# ======================================================================

[event_store]
json_log_dir = "./logs"
sqlite_path = "./events.db"
retention_days = 30

# ======================================================================
# Sources (publish only)
# ======================================================================

# Marketplace exec-source: run any shell command on an interval
[[sources]]
name = "timer"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "5000"]
enabled = true
publishes = ["timer.tick"]

# ======================================================================
# Handlers (subscribe and publish)
# ======================================================================

# TypeScript handler (via Deno)
[[handlers]]
name = "filter"
path = "deno"
args = ["run", "--allow-env", "--allow-net=unix", "/home/user/my-project/filter.ts"]
enabled = true
subscribes = ["timer.tick"]
publishes = ["timer.filtered", "filter.processed"]

# ======================================================================
# Sinks (subscribe only)
# ======================================================================

# Python sink (via uv or python3)
[[sinks]]
name = "console"
path = "python3"
args = ["/home/user/my-project/console.py"]
enabled = true
subscribes = ["timer.filtered", "filter.processed", "system.started.*"]
```

## Engine Section

```toml
[engine]
name = "emergent"              # Instance name (used in socket path)
socket_path = "auto"           # Socket location
api_port = 8891                # HTTP API port (0 to disable)
api_allowed_hosts = []         # Extra host names the HTTP API answers to
max_connections = 1024         # Concurrent IPC connections the engine accepts
shutdown_drain_ms = 500        # Voluntary-exit window per shutdown phase
shutdown_grace_ms = 2000       # Post-SIGTERM window before SIGKILL
startup_ready_timeout_ms = 5000 # Deadline for one startup tier to reach the engine
enforce_declarations = "off"   # Whether declarations bind: off, warn, strict
```

| Option | Default | Description |
|--------|---------|-------------|
| `name` | `"emergent"` | Engine instance name |
| `socket_path` | `"auto"` | `"auto"` for XDG-compliant path, or explicit path like `"/tmp/emergent.sock"` |
| `api_port` | `8891` | HTTP API port for topology queries. Set to `0` to disable. |
| `api_allowed_hosts` | `[]` | From 0.14.0. Host names the HTTP API answers to, beyond an IP literal and `localhost`. |
| `max_connections` | unset | Maximum concurrent IPC connections. Leave it out to keep what acton-reactive resolves. |
| `shutdown_drain_ms` | `500` | How long a shutdown phase waits for its primitives to exit on the `system.shutdown` broadcast alone, before SIGTERM. Sources skip this window because they cannot subscribe. |
| `shutdown_grace_ms` | `2000` | How long a shutdown phase waits after SIGTERM before sending SIGKILL to whatever is still running. |
| `startup_ready_timeout_ms` | `5000` | How long startup waits for one tier of primitives to reach the engine before starting the next. `0` disables the wait. |
| `enforce_declarations` | `"off"` | From 0.14.0. Whether a primitive's `publishes` and `subscribes` lists bind it. `"off"`, `"warn"` or `"strict"`. |

**Startup timing:** `startup_ready_timeout_ms` is a deadline, not a sleep. The
engine leaves a tier the moment every primitive in it that declares `subscribes`
has reached it over IPC, which for an all-Rust topology is a few milliseconds.
Only a primitive that never connects costs the full deadline, and at the
deadline the engine names it in a warning and starts the next tier anyway. Raise
it for a runtime that is genuinely slow to start; set it to `0` to skip the wait
entirely and accept that early events can be missed.

What the engine waits for is a subscribe it saw authorized, not a guess. The
engine's IPC security policy is told about every subscribe before acton
registers it, and startup uses that as its readiness signal, so a primitive
counts as ready whether it asks the engine for its configured `subscribes` or
passes its topics in code. Registering that observer is enough on its own to
install the policy, so this works with `enforce_declarations = "off"`.

Naming the primitive behind a subscribe is the part that is still incomplete.
Until the engine can resolve a peer to the process it spawned, the policy
reports only the peer's kernel pid, and startup matches that against the pids
of its own children. That is exact for a primitive the engine launched
directly. A primitive running behind a wrapper that forks (`uv run`, for
instance) connects from a grandchild whose pid the engine does not know, so it
cannot be named and costs its tier the deadline. So does one whose platform
reports no pid at all.

**Shutdown timing:** both windows are deadlines, not sleeps. A phase moves on the
moment every one of its primitives has exited, so a topology of well-behaved
primitives shuts down in well under a second. A primitive that ignores SIGTERM
is SIGKILLed at the grace deadline, together with anything it spawned, and the
engine logs a warning naming it. Raise `shutdown_grace_ms` for primitives that
legitimately need longer to flush.

**If the engine dies instead of shutting down** (SIGKILL, or the abort a release
build takes on panic), none of that timing applies. From 0.14.0 the
primitives stop anyway: on Linux the engine arms a parent-death SIGTERM in each
child before exec, and on every platform a primitive sees its IPC connection
reach EOF, which ends a Handler's or Sink's subscription stream and, in the Rust
SDK, trips the same shutdown signal `run_source` already gives a Source. On
0.10.10 and earlier the primitives were orphaned and kept running until killed by
hand (Govcraft/emergent#56).

**Connection limit:** every enabled primitive opens exactly one IPC connection
at startup and holds it for the life of its process, so a topology of 20
primitives needs 20 connections plus a little headroom. The ceiling comes from
acton-reactive, which resolves it from `$XDG_CONFIG_HOME/acton/ipc.toml` if that
file sets `[limits] max_connections`, and otherwise from its own default. Set
`[engine].max_connections` to override both from `emergent.toml`, and leave the
key out to accept whatever acton resolves.

On engine 0.10.10 and earlier the engine never looked at the limit. A topology
larger than the ceiling started anyway: the primitives that lost the race to
connect were dropped, `/api/topology` still reported them as `running`, and
nothing said why they were doing no work. From 0.14.0 the engine refuses to
start when the limit cannot cover every enabled primitive plus 4 reserved
connections, and the error names the limit, the primitive count and the key to
raise. The 4 cover one restarting primitive, which can briefly hold both its old
and its new connection, and transient clients such as a CLI query or the
topology viewer, which reach `system.request.topology` over the same socket. The
check runs against the limit that actually took effect, so it catches a ceiling
set in `ipc.toml` as readily as one set in `emergent.toml`.

**Declaration enforcement:** a primitive's `publishes` and `subscribes` lists
describe the topology, and up to engine 0.10.10 they were advisory. The broker
stored and forwarded whatever a client sent and the IPC listener applied
whatever subscription a client asked for, so a topic the code used but the TOML
never declared worked anyway, and the declaration quietly stopped describing the
system. From 0.14.0, `[engine].enforce_declarations` decides whether the lists
bind:

| Value | Effect |
|-------|--------|
| `"off"` (default) | Nothing is checked. Exactly the 0.10.10 behavior, and the engine does not install a security policy at all. |
| `"warn"` | An operation outside the declarations logs at WARN, naming the primitive, the operation and the message type. It still goes through. |
| `"strict"` | The same log line, and the publish is refused: it is neither stored nor forwarded, `publish_ack` fails with an `ACCESS_DENIED` error carrying the engine's explanation, and the engine emits `system.error.<name>` describing the rejection. Subscriptions are checked in neither mode yet, for the reason under "Which name a check is made against" below. |

The default is `"off"` because turning enforcement on can stop messages a
working topology depends on. The shipped example configurations are clean under
`"strict"`, but a topology whose declarations were written before enforcement
existed usually is not. The `exec` handler is the common case: configured the
way its documented example is written, declaring only its `--publish-as` topic,
it violates on every failure because its `--error-as` topic (default
`exec.error`) is undeclared. Run `"warn"` first, read the log, add the topics it
names to `publishes` or `subscribes`, then move to `"strict"`.

Matching follows the same rule as subscriptions everywhere else: a declared
entry is either an exact message type or a prefix ending in a single trailing
`*`, so `publishes = ["metrics.*"]` permits `metrics.cpu`.

Some topics are protocol rather than topology, and are always allowed on the
operation they belong to, for every client:

| Topic | Operation |
|-------|-----------|
| `system.request.subscriptions` | publish |
| `system.request.topology` | publish |
| `system.response.subscriptions` | subscribe |
| `system.response.topology` | subscribe |
| `system.shutdown` | subscribe |

Every SDK publishes the requests and subscribes to the responses on your
primitive's behalf, before your code runs, and `system.shutdown` is how a
primitive learns to stop. Holding a primitive to its TOML on those would refuse
every primitive at startup. The engine's own `system.*` lifecycle events are
produced by the engine, not by a primitive, and are never checked.

**Which name a check is made against.** A message carries a `source` field, and
that is the name the engine checks it under. The client writes that field
itself, so a check catches every honest mistake, which is what a drifted
declaration is, and catches no lie at all. The engine says so at startup
whenever enforcement is on:

```
WARN Declarations are checked against self-reported names: publishes are held
     to the source on the message, subscribes are not checked
```

Three consequences follow, and all three are worth knowing before you rely on
`"strict"`:

- A client can publish under any configured primitive's name, and the engine
  will hold it to that primitive's declarations rather than refuse it.
- A refusal is attributed to the name on the message, so `system.error.<name>`
  can name a primitive that did nothing wrong. The WARN line beside it carries
  the peer pid and `identity.trusted=false`, which is the tell.
- A subscribe frame carries no `source` at all, so there is no name to check a
  subscription under and subscriptions go through unchecked. Refusing them
  instead would stop every handler and sink at startup.

Binding a connection to the primitive that opened it, which closes all three, is
tracked separately (Govcraft/emergent#24). Enforcement is written against that
binding already: when it lands, the checks above start using the engine's own
answer instead of the client's, and subscriptions start being checked, without
a configuration change.

**What enforcement is for.** It keeps the TOML an accurate description of the
system: a topic the code uses but the declaration never mentioned is named in a
log line, and in `"strict"` it stops working, which is what makes anyone fix it.
It is not an authentication boundary and does not try to be one. A client the
engine did not spawn is admitted, and under `"strict"` it is held to the same
declarations as anything else claiming that name; claiming no name at all leaves
it only the protocol questions, which is enough for a CLI or a topology viewer.

Treat access to the Unix socket as the outer trust boundary. Enforcement is what
keeps a topology honest inside it.

**The rejection a client sees.** In `"strict"` mode the refusal carries the
engine's own sentence:

```
publish_ack ERR   'proof' tried to publish 'proof.undeclared', which is not in its declared publishes list
```

A subscribe request, once it is checked, is applied all or nothing, so a batch
containing one undeclared topic is refused whole and the denial names that
topic.

The same sentence is in the engine log and in the `system.error.<name>` payload:

```json
{"primitive":"proof","operation":"publish","message_type":"proof.undeclared",
 "reason":"'proof' tried to publish 'proof.undeclared', which is not in its declared publishes list",
 "mode":"strict"}
```

A sink already subscribed to `system.error.*` sees rejections without
subscribing to anything new. A rejection on a message that named no source has
no primitive to attribute an event to, so it stays in the log, where the peer
pid is.

**Which hosts the HTTP API answers to:** the API binds `127.0.0.1` and sends no
`Access-Control-Allow-Origin`, which keeps other machines and other origins out.
Neither stops DNS rebinding. A page on `attacker.example` re-resolves its own
name to `127.0.0.1`, and from then on the browser treats the API as that page's
own origin and hands it the reply: every primitive's name, kind, state, pid and
declared topics. Up to engine 0.10.10 this worked, and the demonstration was one
line:

```
$ curl -H 'Host: attacker.example' http://127.0.0.1:8891/api/topology
200  {"primitives":[{"name":"emergent-engine", ...
```

The one thing an attacker cannot choose is the name the browser puts in the
request. So from 0.14.0 the API answers only when every host the request names
is one of:

- an IP literal, such as `127.0.0.1`, `192.168.1.20` or `[::1]`, because a
  browser sends the address it connected to and an address cannot be rebound
- `localhost`
- a name in `[engine].api_allowed_hosts`

Anything else gets `421 Misdirected Request` with a body naming the key, and a
WARN line in the engine log. The port is never compared, so a port forward or a
container published as `http://localhost:8891` keeps working unchanged.

```toml
[engine]
api_allowed_hosts = ["emergent.internal"]
```

That list is for a reverse proxy that forwards its own public name. Write a host
name and nothing else: a value with a scheme, a port, a path or a `*` is a load
error naming the value, because it would sit in the list and never match. A name
is stored the way a browser sends it, so `APP.Example.` and `münchen.example`
match the `app.example` and `xn--mnchen-3ya.example` that arrive.

A request that names no host at all is refused, as is one that names two: hyper
hands over a duplicate `Host` header unjoined, and it does not reconcile an
absolute-form request line with the `Host` beside it. `axum::serve` also speaks
HTTP/2 over cleartext, where there is no `Host` header and the authority is a
pseudo-header, so `curl --http2-prior-knowledge` is checked by the same rule as
everything else.

This is the rule the `sse-sink` and `topology-viewer` primitives already apply
(Govcraft/emergent-primitives#16), and the engine's table test mirrors theirs row
for row so the two cannot drift apart.

The API is read-only, so what this protects is disclosure, not control. Treat
access to the port as the outer boundary.

**Publish rate limit:** the same acton-reactive layer also rate limits each IPC
connection to 100 messages per second with a burst of 50, and there is no
`emergent.toml` key for it. Because a primitive holds exactly one connection,
that is a per-primitive budget. A source that publishes faster than its budget
has its excess messages refused by the engine and never delivered. Raise the
limit in `$XDG_CONFIG_HOME/acton/ipc.toml` under `[rate_limit]`
(`requests_per_second`, `burst_size`), have the primitive batch several records
into one message, or have it publish with an acknowledgment so it cannot outpace
the engine.

The refusal is visible from the primitive side. From 0.14.0 the Rust
SDK logs it at `WARN` with the engine's error text and the message type and
counts it for the caller; the Go, Python and TypeScript SDKs log the unmatched
ERROR frame at error level. On 0.13.1 and earlier the Rust SDK's fire-and-forget
`publish` returned success and the refusal was dropped at `trace` level, so the
loss was silent (Govcraft/emergent#65).

`wire_format` is accepted but selects nothing: IPC is always MessagePack. On engine 0.10.10 and earlier the key was silently inert and the startup line reported the value you set. From 0.14.0 the engine warns at startup that the key has no effect and the ready line no longer names a wire format. Leave it out. To read events in a human-readable form, read the JSON event log.

**Socket path resolution:**

- `"auto"`: Uses XDG base directories (`$XDG_RUNTIME_DIR/emergent/emergent.sock`)
- Explicit: Use any valid Unix socket path

## Event Store Section

```toml
[event_store]
json_log_dir = "./logs"
sqlite_path = "./events.db"
retention_days = 30
```

| Option | Default | Description |
|--------|---------|-------------|
| `json_log_dir` | `"./logs"` | Directory for append-only JSON log files (one per day) |
| `sqlite_path` | `"./events.db"` | Path to SQLite database for structured queries |
| `retention_days` | `30` | Days of events to keep. `0` keeps everything |

Both paths support `"auto"` for XDG data directory placement.

**Retention:** on engine 0.10.10 and earlier `retention_days` was parsed and never enforced, so both stores grew without bound. From 0.14.0 the engine prunes at startup and once a day afterwards: SQLite rows older than the window are deleted, and `events-YYYY-MM-DD.jsonl` files dated before the window are removed. The day at the edge of the window is kept, and files that are not rotated event logs are never touched. Each pass logs what it removed. `retention_days = 0` disables pruning and keeps every event, which the engine states at startup.

## Sources

Sources bring data into the system. They can only publish -- they cannot subscribe.

```toml
[[sources]]
name = "timer"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "5000"]
enabled = true
publishes = ["timer.tick"]
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments (array of strings) |
| `enabled` | No | `true` (default) or `false` to disable |
| `publishes` | Yes | Message types this source will emit |
| `env` | No | Extra environment variables (key-value map) |
| `restart` | No | Supervision policy. See [Restart Policy](#restart-policy). |

**Path resolution:** Tilde expansion (`~/bin/app`), bare command lookup via PATH (`path = "python3"`), and `"auto"` XDG paths are all supported.

## Handlers

Handlers transform data. They subscribe to events, process them, and publish new events.

```toml
[[handlers]]
name = "filter"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "timer.tick", "--publish-as", "timer.filtered", "--", "jq", "."]
enabled = true
subscribes = ["timer.tick"]
publishes = ["timer.filtered"]
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments |
| `enabled` | No | `true` (default) or `false` |
| `subscribes` | Yes | Message types to receive |
| `publishes` | Yes | Message types this handler will emit |
| `unwrap_stdout` | No | When `true`, the SDK automatically extracts and parses the `.stdout` field from exec-source's `{command, stdout, exit_code}` envelope before delivering messages. Eliminates the need for a dedicated unwrap handler. |
| `env` | No | Extra environment variables (key-value map) |
| `restart` | No | Supervision policy. See [Restart Policy](#restart-policy). |

## Sinks

Sinks consume data. They subscribe to events but cannot publish.

```toml
[[sinks]]
name = "console"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "timer.filtered", "--", "jq", "."]
enabled = true
subscribes = ["timer.filtered", "system.started.*"]
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments |
| `enabled` | No | `true` (default) or `false` |
| `subscribes` | Yes | Message types to receive |
| `unwrap_stdout` | No | When `true`, the SDK automatically extracts and parses the `.stdout` field from exec-source's envelope before delivering messages. |
| `env` | No | Extra environment variables (key-value map) |
| `restart` | No | Supervision policy. See [Restart Policy](#restart-policy). |

## Restart Policy

By default a primitive that exits stays down until the engine is restarted. Set
`restart` on any source, handler or sink to have the engine respawn it.

```toml
[[handlers]]
name = "enricher"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "order.placed", "--", "jq", "-c", "."]
subscribes = ["order.placed"]
restart = "on-failure"
restart_backoff_ms = 500       # Delay before the first retry
restart_max_backoff_ms = 30000 # Ceiling for the doubling backoff
restart_max_retries = 5        # Restarts allowed inside the window
restart_window_ms = 60000      # Sliding window for counting restarts
```

| Field | Default | Description |
|-------|---------|-------------|
| `restart` | `"never"` | `"never"`, `"on-failure"` or `"always"` |
| `restart_backoff_ms` | `500` | Delay before the first restart attempt |
| `restart_max_backoff_ms` | `30000` | Ceiling for the backoff, which doubles each attempt |
| `restart_max_retries` | `5` | Restarts allowed inside `restart_window_ms` |
| `restart_window_ms` | `60000` | Sliding window over which restarts are counted |

| Policy | Restarts on |
|--------|-------------|
| `never` | Nothing. The default, and the behaviour of every earlier release. |
| `on-failure` | A non-zero exit code, or death by a signal other than SIGTERM. |
| `always` | Any exit, clean or not. |

The delay doubles with each attempt inside the window (500 ms, 1 s, 2 s, ...)
up to `restart_max_backoff_ms`. Once `restart_max_retries` restarts have
happened inside `restart_window_ms`, the primitive is left in the `Failed`
state and the engine emits `system.error.<name>` with an error of
`"Restarts exhausted: N attempts within W ms"`. Attempts age out of the window,
so a primitive that fails rarely is restarted indefinitely.

A restart is never attempted while the engine is shutting down, whatever the
policy says.

Any value of `restart` other than the three above is a configuration error
naming both the primitive and the value, and the engine refuses to start.

### Reacting to Restarts

Each successful respawn emits `system.restarted.<name>`, alongside the
`system.error.<name>` that reported the exit:

```toml
[[sinks]]
name = "alerts"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "system.restarted.*", "--", "jq", "-c", "."]
subscribes = ["system.restarted.*"]
```

The payload carries the same fields as `system.started.<name>` plus
`restart_attempt`, the 1-based attempt number inside the current window:

```json
{
  "name": "enricher",
  "kind": "handler",
  "pid": 48211,
  "subscribes": ["order.placed"],
  "restart_attempt": 2
}
```

## Subscription Patterns

A subscription is either an exact message type or a prefix ending in a single
`*`:

```toml
subscribes = [
    "timer.tick",           # Exact match
    "system.started.*",     # Matches system.started.timer, system.started.filter, etc.
    "system.error.*",       # All error events
    "*",                    # Everything the engine publishes
]
```

The `*` must be the last character. `system.*.error` could never match, so the
engine refuses to start and names the primitive and the topic rather than
running a primitive that would receive nothing.

Overlapping entries deliver one copy each: a primitive subscribing to both
`timer.tick` and `timer.*` receives a single `timer.tick`.

Wildcard routing arrived from 0.14.0. On 0.10.10 and earlier a
subscription containing `*` was accepted and then never delivered, so configs
written for those releases name every type.

## Multi-Language Primitives

The engine spawns each primitive as a child process. The `path` field can point to a compiled binary, a language runtime, or any executable.

### Marketplace Primitives (exec)

```toml
[[handlers]]
name = "transform"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "input.event", "--publish-as", "output.event", "--", "jq", ".data"]
```

### Rust (compiled binary)

```toml
[[handlers]]
name = "filter"
path = "/home/user/my-project/target/release/filter"
```

### TypeScript (Deno)

```toml
[[sinks]]
name = "console_color"
path = "deno"
args = [
    "run",
    "--allow-env",
    "--allow-read",
    "--allow-write",
    "--allow-net=unix",
    "/home/user/my-project/console_color.ts"
]
```

### Python

```toml
# Using python3 directly
[[sources]]
name = "webhook"
path = "python3"
args = ["/home/user/my-project/webhook.py", "--port", "8008"]

# Using uv for dependency management
[[sources]]
name = "webhook"
path = "uv"
args = ["run", "--project", "/home/user/my-project", "python", "/home/user/my-project/webhook.py"]
```

### Go (compiled binary)

```toml
[[sources]]
name = "timer-go"
path = "./timer-go"
args = ["--interval", "3000"]
publishes = ["timer.tick"]
```

## Environment Variables

The engine sets these environment variables for each primitive:

| Variable | Description |
|----------|-------------|
| `EMERGENT_SOCKET` | Path to Unix socket for IPC |
| `EMERGENT_NAME` | Primitive's configured name |
| `EMERGENT_API_PORT` | HTTP API port for topology queries |
| `EMERGENT_PUBLISHES` | Comma-separated list of publish types from config |
| `EMERGENT_SUBSCRIBES` | Comma-separated list of subscribe types from config |

Additionally, the engine forwards the parent process's full environment to all child primitives. This is how secrets and configuration reach your tools.

## Startup Order

The engine starts primitives in this order:

1. **Sinks** -- started first so they are ready to receive messages
2. **Handlers** -- started next so they can process messages
3. **Sources** -- started last so they produce messages only when the pipeline is ready

Within each tier, primitives start in the order they appear in the configuration file. This matters when one primitive depends on another's `system.started.*` event -- the subscriber must appear before the publisher in the config.

From 0.14.0 the engine waits for a tier before starting the next one: it holds until every primitive in the tier that declares `subscribes` has reached it over IPC, then moves on. A primitive that declares no `subscribes`, every source among them, is never waited on. `startup_ready_timeout_ms` bounds the wait; at the deadline the engine logs a warning naming the primitives it never heard from and carries on, so one broken primitive cannot hang startup. A primitive that exits or fails during the wait releases its tier at once.

On 0.10.10 and earlier the engine slept a fixed 50 ms after each primitive and started the next tier regardless, so anything slower than that to subscribe missed the first events (Govcraft/emergent#66). That included every Deno and Python primitive on a cold start.

At shutdown, the order reverses: Sources stop first (no new messages), then Handlers drain, then Sinks consume remaining messages.

## System Events

The engine publishes lifecycle events:

| Event | When |
|-------|------|
| `system.started.<name>` | Primitive connected |
| `system.stopped.<name>` | Primitive disconnected |
| `system.error.<name>` | Primitive failed |
| `system.restarted.<name>` | Primitive respawned by its restart policy |
| `system.shutdown.requested` | Shutdown requested -- cleanup window before teardown |
| `system.shutdown` | Graceful shutdown in progress (intercepted by SDK) |

Subscribe to monitor:

```toml
[[sinks]]
name = "monitor"
subscribes = ["system.started.*", "system.stopped.*", "system.error.*"]
```

## Disabling Primitives

Set `enabled = false` to disable without removing:

```toml
[[handlers]]
name = "enricher"
path = "/path/to/enricher"
enabled = false  # Temporarily disabled
subscribes = ["event.raw"]
publishes = ["event.enriched"]
```

## Secrets

Never hardcode tokens, keys, or credentials in TOML configuration files. The engine forwards the parent process's full environment to all child primitives, so shell variable expansion works inside `sh -c` commands:

```toml
[[sources]]
name = "slack-connect"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = [
    "--shell", "sh",
    "--command", "curl -s -X POST https://slack.com/api/apps.connections.open -H \"Authorization: Bearer $SLACK_APP_TOKEN\" | jq -c '{url: .url}'"
]
publishes = ["exec.output"]
```

Set secrets before starting the engine:

```bash
# Option 1: export directly
export SLACK_APP_TOKEN="xapp-..."
export SLACK_BOT_TOKEN="xoxb-..."
emergent --config ./emergent.toml

# Option 2: source a .env file (add .env to .gitignore)
source .env
emergent --config ./emergent.toml
```

### Production: systemd credentials

For unattended services on Linux, use `systemd-creds` to encrypt secrets to the machine's TPM or host key. Decrypted values exist only as files in a tmpfs mount scoped to the service lifetime -- they never appear in environment variables or on disk.

Encrypt each secret to a credential file:

```bash
# For user services (--user)
echo -n "xoxb-..." | systemd-creds encrypt --user --name=SLACK_BOT_TOKEN - ~/.config/emergent/secrets/slack-bot-token.cred
echo -n "xapp-..." | systemd-creds encrypt --user --name=SLACK_APP_TOKEN - ~/.config/emergent/secrets/slack-app-token.cred

# For system services (omit --user)
echo -n "xoxb-..." | systemd-creds encrypt --name=SLACK_BOT_TOKEN - /etc/emergent/secrets/slack-bot-token.cred
```

Load the credentials in your service unit:

```ini
[Service]
LoadCredentialEncrypted=SLACK_BOT_TOKEN:%h/.config/emergent/secrets/slack-bot-token.cred
LoadCredentialEncrypted=SLACK_APP_TOKEN:%h/.config/emergent/secrets/slack-app-token.cred
```

The engine forwards `$CREDENTIALS_DIRECTORY` to all child primitives. Read secrets from credential files in your TOML args using `$(cat ...)`:

```toml
args = [
    "--shell", "sh",
    "--command", "curl -s -X POST https://slack.com/api/apps.connections.open -H \"Authorization: Bearer $(cat $CREDENTIALS_DIRECTORY/SLACK_APP_TOKEN)\" | jq -c '{url: .url}'"
]
```

See [Local Deployment](local-deployment.md) for the complete systemd setup.

### Production: macOS Keychain

On macOS, store secrets in Keychain and retrieve them in shell commands:

```toml
args = ["--shell", "sh", "--command", "curl -s ... -H \"Authorization: Bearer $(security find-generic-password -s SLACK_BOT_TOKEN -w)\" ..."]
```

### Security considerations

- **Process environment**: Other users on a shared system can read process environment variables via `/proc/<pid>/environ`. On single-user machines this is not a concern.
- **Child inheritance**: All primitives in the pipeline inherit the engine's full environment. A compromised primitive could read any secret in the environment.
- **Shell history**: Use `source .env` rather than inline `export` to keep secrets out of shell history.

## Validation

The engine validates configuration at startup:

- All primitive names must be unique
- Every primitive name must start with a lowercase letter and use only `a-z`, `0-9`, `-`, `_`, at most 64 characters, because the engine builds `system.started.<name>` from it. On engine 0.10.10 and earlier this was not checked and an invalid name aborted the engine once that primitive started; from 0.14.0 the load fails with an error naming the primitive and the rule
- Paths must exist
- Every `subscribes` entry must be an exact message type or a prefix ending in a single trailing `*`. A wildcard anywhere else, such as `system.*.error`, is a load error (from 0.14.0)
- `restart` must be one of `never`, `on-failure` or `always` (from 0.14.0)
- Unknown keys are a load error that names the key and its table (from 0.14.0). That is what rejects `subscribes` on a source and `publishes` on a sink, since neither table has that key

`subscribes` and `publishes` may be empty or omitted. The engine does not require them, and an empty `subscribes` on a handler or sink loads and receives nothing.
