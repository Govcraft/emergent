# Emergent Configuration Reference

Configuration is stored in TOML format, typically `emergent.toml`. The engine
looks for `--config/-c FILE`, then `./emergent.toml`, then
`~/.config/emergent/emergent.toml`. `--socket/-s PATH` overrides `socket_path`,
and `--verbose/-v` logs to stderr instead of
`~/.local/share/emergent/<engine.name>/emergent.log`.

On engine 0.10.10 and earlier unknown keys were ignored without a warning, so a
typo such as `subscribe =` produced a primitive that loaded and received nothing.
After 0.10.10 an unknown key is a load error naming the key, the keys its table
accepts, and the file. On an older engine, check spelling first when a primitive
is silent.

## Complete Example

```toml
# =============================================================================
# Engine Settings
# =============================================================================

[engine]
# Name of this engine instance (used in socket path and logging)
name = "emergent"

# Socket path for IPC connections
# "auto" - XDG-compliant automatic path (recommended)
# "/path/to/socket" - Explicit path
socket_path = "auto"

# HTTP API port (default: 8891, set 0 to disable)
# Serves GET /api/topology on 127.0.0.1: every enabled primitive with its kind, publishes, subscribes
api_port = 8891

# =============================================================================
# Event Store Settings
# =============================================================================

[event_store]
# Directory for JSON log files (append-only, one file per day)
# "auto": uses XDG data directory
json_log_dir = "auto"

# Path to SQLite database for structured event storage
# "auto": uses XDG data directory
sqlite_path = "auto"

# Days of events to keep. 0 keeps everything
retention_days = 30

# =============================================================================
# Sources - Data Ingress (publish only, no subscribe)
# =============================================================================

[[sources]]
name = "ticker"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "3000"]
publishes = ["exec.output"]

[[sources]]
name = "webhook"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--host", "127.0.0.1", "--port", "8080"]   # secret: export HTTP_SOURCE_SECRET before starting the engine
publishes = ["http.request"]

# =============================================================================
# Handlers - Transformation (subscribe + publish)
# =============================================================================

[[handlers]]
name = "transform"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "exec.output", "--publish-as", "data.transformed", "--", "jq", "-c", ". + {transformed: true}"]
subscribes = ["exec.output"]
publishes = ["data.transformed"]

[[handlers]]
name = "ws"
path = "~/.local/share/emergent/primitives/bin/websocket-handler"
args = ["--prefix", "ws"]
subscribes = ["ws.connect", "ws.send", "ws.disconnect"]
publishes = ["ws.connected", "ws.frame", "ws.closed", "ws.disconnected", "ws.error"]

# =============================================================================
# Sinks - Data Egress (subscribe only, no publish)
# =============================================================================

[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "data.transformed", "--", "jq", "."]
subscribes = ["data.transformed"]

[[sinks]]
name = "dashboard"
path = "~/.local/share/emergent/primitives/bin/sse-sink"
args = ["--port", "8081"]
subscribes = ["data.transformed"]

[[sinks]]
name = "topology"
path = "~/.local/share/emergent/primitives/bin/topology-viewer"
args = ["--port", "8009"]
subscribes = ["system.started.ticker", "system.stopped.ticker", "system.error.ticker"]
```

## Section Reference

### [engine]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | `"emergent"` | Engine instance name |
| `socket_path` | String | `"auto"` | Unix socket path (`"auto"` for XDG default) |
| `api_port` | Integer | `8891` | HTTP API port (`0` to disable) |
| `max_connections` | Integer | unset | After 0.10.10. Maximum concurrent IPC connections. Unset keeps what acton-reactive resolves, from `$XDG_CONFIG_HOME/acton/ipc.toml` or its own default. Setting it overrides both |
| `shutdown_drain_ms` | Integer | `500` | After 0.10.10. How long a shutdown phase waits for its handlers or sinks to exit on the `system.shutdown` broadcast before SIGTERM. Sources skip it |
| `shutdown_grace_ms` | Integer | `2000` | After 0.10.10. How long a phase waits after SIGTERM before it SIGKILLs whatever is still running |
| `enforce_declarations` | String | `"off"` | After 0.10.10. Whether a primitive's `publishes` and `subscribes` lists bind it: `"off"`, `"warn"` or `"strict"` |

A primitive's `publishes` and `subscribes` lists were advisory up to engine
0.10.10: the broker stored and forwarded whatever a client sent, and the IPC
listener applied whatever subscription a client asked for. After 0.10.10,
`enforce_declarations` decides whether they bind. `"warn"` logs an operation
outside the declarations at WARN, naming the primitive, the operation and the
message type, and lets it through. `"strict"` also refuses it: a publish is
neither stored nor forwarded and `publish_ack` fails, a subscribe is not
applied, the client gets an `ACCESS_DENIED` error carrying the engine's own
sentence, and the engine emits `system.error.<name>` with the same reason.
Matching uses the same rule as subscriptions, an exact type or a single trailing
`*`. A subscribe batch is applied all or nothing, so one undeclared topic
refuses the batch and the denial names it.

Protocol topics are always allowed, on the operation they belong to: publishing
`system.request.subscriptions` or `system.request.topology`, and subscribing to
`system.response.subscriptions`, `system.response.topology` or
`system.shutdown`. Every SDK does all five on the primitive's behalf before its
code runs, so enforcing declarations over them would refuse every primitive at
startup. The engine's own `system.*` lifecycle events never go through the
check.

The default is `"off"` because enforcement can stop messages a working topology
depends on. The shipped example configs are clean under `"strict"`, but an
`exec` handler configured the way its documented example is written violates on
every failure, because its `--error-as` topic (default `exec.error`) is not in
`publishes`. Run `"warn"`, fix what it names, then go `"strict"`.

Be precise about the guarantee. The engine identifies the primitive when the
connection is accepted, from the peer pid the kernel reports, walking up the
process ancestry to a pid it spawned so that a primitive behind a wrapper such
as `uv` still resolves. That name is not the `source` field the client writes,
so a message claiming another primitive's name is refused too. What is not
covered: a client the engine did not spawn is still admitted, and under
`"strict"` may do only what any unidentified client may do, which is ask the
engine the protocol questions, enough for a CLI or a topology viewer and not
enough to inject traffic; pids can be reused, and the engine does not yet bind a
connection to a child process instance or revoke it when that child exits
(Govcraft/emergent#24); and where `/proc` is unavailable the ancestry walk
cannot run, so a primitive behind a wrapper resolves as unidentified. Access to
the Unix socket is the outer trust boundary.

Every enabled primitive holds one IPC connection for the life of its process,
so the connection ceiling is a hard cap on topology size. On engine 0.10.10 and
earlier the engine never checked it: an oversized topology started, the
primitives that lost the race to connect were dropped, and `/api/topology` still
called them `running`. After 0.10.10 the engine refuses to start unless the
effective limit covers every enabled primitive plus 4 reserved connections, one
for a restarting primitive holding two at once and three for CLI or
topology-viewer queries over the same socket. The error names the limit, the
primitive count and the required total. The check reads the limit that actually
took effect, so a ceiling set in `ipc.toml` is caught as readily as one set in
`emergent.toml`.

Leave `wire_format` unset. The key still parses (`"messagepack"` or `"json"`) but has no effect: IPC is always MessagePack. On engine 0.10.10 and earlier it was silently inert and the ready line echoed it back; after 0.10.10 setting it earns a warning at startup and the ready line no longer names a format. For human-readable inspection, read the event store's JSON logs.

### [event_store]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `json_log_dir` | Path | `"auto"` | Directory for JSON log files. `"auto"` is `~/.local/share/emergent/<engine.name>/logs` on Linux |
| `sqlite_path` | Path | `"auto"` | SQLite database path. `"auto"` is `~/.local/share/emergent/<engine.name>/events.db` on Linux |
| `retention_days` | Integer | `30` | Days of events to keep. `0` keeps everything |

Both stores are always on. `~` is **not** expanded in `json_log_dir`,
`sqlite_path`, or `socket_path`; only a primitive's `path` gets tilde expansion.

On engine 0.10.10 and earlier `retention_days` was parsed and nothing pruned
either store, so a fast source filled the disk. After 0.10.10 the engine prunes
at startup and once a day: SQLite rows older than the window are deleted, and
`events-YYYY-MM-DD.jsonl` files dated before it are removed, keeping the day at
the edge of the window and never touching files that are not rotated event logs.
Each pass logs what it removed. `retention_days = 0` disables pruning and keeps
everything, which the engine states at startup.

The JSON log is one file per UTC day, `events-YYYY-MM-DD.jsonl`, one line per
event: `{"timestamp": "<RFC3339>", "message": <envelope>}`.

The SQLite table is `events(id, message_type, source, correlation_id,
causation_id, timestamp_ms, payload_json, metadata_json, created_at)`, indexed on
`message_type`, `timestamp_ms`, `source`, and `correlation_id`. Tracing a run is
one query:

```bash
sqlite3 ~/.local/share/emergent/<engine.name>/events.db \
  "SELECT message_type, source, payload_json FROM events
   WHERE correlation_id = 'cor_...' ORDER BY timestamp_ms"
```

### [[sources]]

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Unique across sources, handlers, and sinks combined. Lowercase letter first, then `a-z 0-9 - _`, at most 64 characters, because the name becomes the last segment of `system.started.<name>`. On engine 0.10.10 and earlier the config loader did not check this, and an invalid name (a space, an uppercase letter) aborted the engine with exit 134 the moment that primitive started, leaving the child it just spawned running as an orphan (Govcraft/emergent#42). After 0.10.10 the loader rejects the name, naming the primitive and the rule, and the engine exits 1 before spawning anything |
| `path` | Path | Yes | Path to executable. See Path Resolution. Must exist for every enabled primitive or the engine refuses to start |
| `args` | Array[String] | No | Command-line arguments |
| `enabled` | Boolean | No | Enable/disable (default: `true`) |
| `publishes` | Array[String] | No | Message types this source publishes. Types use `a-z 0-9 . _ -` with no leading, trailing, or doubled dots. Marketplace primitives read this array and it overrides their topic flags by position |
| `env` | Map[String, String] | No | Environment variables, as literals. See Secrets Management before putting anything here |

### [[handlers]]

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Unique identifier for this handler |
| `path` | Path | Yes | Path to executable |
| `args` | Array[String] | No | Command-line arguments |
| `enabled` | Boolean | No | Enable/disable (default: `true`) |
| `subscribes` | Array[String] | No (default `[]`) | Message types the primitive consumes. It reaches the primitive as `EMERGENT_SUBSCRIBES` and draws the topology graph, but a primitive that calls `subscribe([...])` with its own list ignores it: the exec primitives subscribe to their `-s` flags and `stream-runner` to its topic flags. Keep the two in agreement |
| `publishes` | Array[String] | No | Message types this handler publishes |
| `env` | Map[String, String] | No | Environment variables, as literals. See Secrets Management before putting anything here |
| `unwrap_stdout` | Boolean | No | When `true`, the SDK auto-extracts and parses the `.stdout` field from exec-source's `{command, stdout, exit_code}` envelope before delivering messages (engine sets `EMERGENT_UNWRAP_STDOUT=true` for the primitive) |

### [[sinks]]

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Unique identifier for this sink |
| `path` | Path | Yes | Path to executable |
| `args` | Array[String] | No | Command-line arguments |
| `enabled` | Boolean | No | Enable/disable (default: `true`) |
| `subscribes` | Array[String] | No (default `[]`) | Message types the primitive consumes. It reaches the primitive as `EMERGENT_SUBSCRIBES` and draws the topology graph, but a primitive that calls `subscribe([...])` with its own list ignores it: the exec primitives subscribe to their `-s` flags and `stream-runner` to its topic flags. Keep the two in agreement |
| `env` | Map[String, String] | No | Environment variables, as literals. See Secrets Management before putting anything here |
| `unwrap_stdout` | Boolean | No | Same as for handlers: auto-unwrap exec-source's stdout envelope |

### Restart policy (any primitive, after 0.10.10)

Engine 0.10.10 and earlier never restart a primitive and do not know these keys. After 0.10.10 every `[[sources]]`, `[[handlers]]` and `[[sinks]]` block also takes:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `restart` | String | `"never"` | `"never"`, `"on-failure"` (non-zero exit, or death by a signal other than SIGTERM) or `"always"` (any exit). Any other value stops the engine at config load with an error naming the primitive |
| `restart_backoff_ms` | Integer | `500` | Delay before the first restart. Doubles each attempt |
| `restart_max_backoff_ms` | Integer | `30000` | Ceiling for the doubling |
| `restart_max_retries` | Integer | `5` | Restarts allowed inside the window. One more exit after that leaves the primitive failed, with `system.error.<name>` carrying `"Restarts exhausted: N attempts within W ms"` |
| `restart_window_ms` | Integer | `60000` | Sliding window the restarts are counted over. Attempts age out, so a primitive that fails rarely is restarted indefinitely |

Each respawn emits `system.restarted.<name>`. Do not put `restart = "always"` on a one-shot source: exiting is how it finishes, and the policy would turn it into a loop. Reach for `on-failure` on long-running primitives that hold no state worth keeping, and leave the default everywhere else.

## Subscriptions are literal names or terminal-wildcard prefixes

A subscription either matches one message type character for character, or ends
in a single `*` and matches every type that starts with the text before it.
`system.error.*` reaches `system.error.poll-issues`, `timer.*` reaches
`timer.tick`, and `*` reaches everything the engine publishes, including types
that first appear later in the run.

```toml
# Naming each failure:
subscribes = ["system.error.poll-issues", "system.error.score-severity"]
```

```toml
# Watching the same two, and any primitive added later:
subscribes = ["system.error.*"]
```

Overlapping entries are free: a sink listing both `timer.tick` and `timer.*`
receives one copy of each `timer.tick`.

The star is terminal. `system.*.error` and `*.tick` could never match, so the
engine refuses to load the config and names the primitive and the topic. Write
the prefix the star follows instead.

**On engine 0.10.10 and earlier there was no wildcard routing.**
`system.error.*` and `timer.*` loaded without complaint and then received
nothing, which made it the quietest way to build a sink that never fires. A
topology that lists every type explicitly is still correct on every release,
and is the portable choice if the same file has to run against an older engine.

## Path Resolution

The engine resolves `path` in three ways:

1. **Tilde expansion**: `~/bin/app` → `/home/user/bin/app`. Only a leading `~`
   or `~/`; `~user/` is not expanded.
2. **Bare command lookup**: `path = "uv"` searches PATH, but only when the value
   contains no `/` and no file of that name exists in the working directory.
3. **"auto" for XDG paths**: Socket and event store paths support `"auto"`.

A relative `path` resolves against the engine's working directory, not the
config file's directory. The marketplace install directory shown throughout,
`~/.local/share/emergent/primitives/bin/`, is the Linux location; on macOS it is
under `~/Library/Application Support/ai.govcraft.emergent/`.

## Socket Path Resolution

When `socket_path = "auto"`:

1. Check for XDG runtime directory (`$XDG_RUNTIME_DIR`)
2. If available: `$XDG_RUNTIME_DIR/{name}.sock`
3. Fallback: `/tmp/{name}.sock`

## Environment Variables Set by Engine

When spawning primitives, the engine sets:

| Variable | Purpose |
|----------|---------|
| `EMERGENT_SOCKET` | Path to the engine's Unix socket |
| `EMERGENT_NAME` | Name of this primitive (from config) |
| `EMERGENT_PUBLISHES` | Comma-separated publish types |
| `EMERGENT_SUBSCRIBES` | Comma-separated subscribe types |
| `EMERGENT_API_PORT` | `[engine].api_port` (`0` when the HTTP API is disabled) |
| `EMERGENT_UNWRAP_STDOUT` | `true` when `unwrap_stdout = true` |

Engine-set variables win over same-named keys in a primitive's `env`.

## Multi-Language Paths

```toml
# Marketplace primitives (installed binaries)
path = "~/.local/share/emergent/primitives/bin/exec-handler"

# Rust (compiled binary)
path = "./target/release/my_handler"

# Python (via uv, recommended)
path = "uv"
args = ["run", "--with", "emergent-client", "my_handler.py"]

# TypeScript (via Deno)
path = "deno"
args = ["run", "--allow-env", "--allow-net=unix", "my_handler.ts"]

# Go (compiled binary)
path = "./my_handler"
```

## Secrets Management

Never put a secret in `emergent.toml`. Primitives inherit the engine's
environment, so export the secret before starting the engine and read it inside
the command:

```toml
[[sinks]]
name = "post-alert"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "alert.raised", "--", "sh", "-c",
        "curl -sf -H \"Authorization: Bearer $API_TOKEN\" -d @- https://example.com/hook"]
subscribes = ["alert.raised"]
```

Three traps:

- **`args` and `env` values are literals.** Nothing expands `$VAR` or `$(...)`
  in them. Expansion happens only inside an explicit `sh -c "..."` argument,
  because then a shell is doing it.
- **An `env` entry replaces the inherited value.** `env = { API_TOKEN = "" }`
  does not mean "pass it through"; it blanks the exported token.
- **Marketplace primitives that take a secret read an environment variable**
  (`HTTP_SOURCE_SECRET`, `TYPESAFE_API_KEY`). Use that, not the flag, so the
  secret stays out of the process table as well as the file.

For production on Linux, a systemd unit for the engine with `EnvironmentFile=`
or `LoadCredentialEncrypted=` keeps the secret out of your shell history too.
