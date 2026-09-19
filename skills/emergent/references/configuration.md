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
publishes = ["ws.connected", "ws.frame", "ws.closed", "ws.error"]

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
| `name` | String | Yes | Unique across sources, handlers, and sinks combined. Lowercase letter first, then `a-z 0-9 - _`, at most 64 characters. The config loader does not check this, and an invalid name (a space, an uppercase letter) aborts the engine with exit 134 the moment that primitive starts, leaving the child it just spawned running as an orphan (Govcraft/emergent#42) |
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

## Subscriptions are exact-match

A subscription matches one message type, character for character. **There is no
wildcard routing.** `system.error.*` and `timer.*` load without complaint and
then receive nothing, which makes this the quietest way to build a sink that
never fires.

List every type explicitly. System events are typed per primitive, so watching
two primitives for failure is two entries:

```toml
subscribes = ["system.error.poll-issues", "system.error.score-severity"]
```

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
