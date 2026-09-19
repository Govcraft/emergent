# Emergent Configuration Reference

Configuration is stored in TOML format, typically `emergent.toml`.

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
# Serves GET /api/topology — returns all primitives with state, publishes, subscribes, PID
api_port = 8891

# =============================================================================
# Event Store Settings
# =============================================================================

[event_store]
# Directory for JSON log files (append-only, one file per day)
# "auto" — uses XDG data directory
json_log_dir = "auto"

# Path to SQLite database for structured event storage
# "auto" — uses XDG data directory
sqlite_path = "auto"

# How many days to retain events before cleanup
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
args = ["--port", "8080", "--secret", "$HTTP_WEBHOOK_SECRET"]
publishes = ["http.request"]
env = { HTTP_WEBHOOK_SECRET = "" }

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
subscribes = ["ws.connect", "ws.send"]
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
subscribes = ["system.started.*", "system.stopped.*", "system.error.*"]
```

## Section Reference

### [engine]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | `"emergent"` | Engine instance name |
| `socket_path` | String | `"auto"` | Unix socket path (`"auto"` for XDG default) |
| `api_port` | Integer | `8891` | HTTP API port (`0` to disable) |

There is no `wire_format` option — IPC is always MessagePack. The option was removed because setting `"json"` silently broke all SDK communication. For human-readable inspection, read the event store's JSON logs.

### [event_store]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `json_log_dir` | Path | `"auto"` | Directory for JSON log files (`"auto"` for XDG data dir) |
| `sqlite_path` | Path | `"auto"` | SQLite database path (`"auto"` for XDG data dir) |
| `retention_days` | Integer | `30` | Days to retain events |

### [[sources]]

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Unique identifier for this source |
| `path` | Path | Yes | Path to executable (supports `~` expansion, bare commands via PATH) |
| `args` | Array[String] | No | Command-line arguments |
| `enabled` | Boolean | No | Enable/disable (default: `true`) |
| `publishes` | Array[String] | No | Message types this source publishes |
| `env` | Map[String, String] | No | Environment variables |

### [[handlers]]

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Unique identifier for this handler |
| `path` | Path | Yes | Path to executable |
| `args` | Array[String] | No | Command-line arguments |
| `enabled` | Boolean | No | Enable/disable (default: `true`) |
| `subscribes` | Array[String] | Yes | Message types to subscribe to |
| `publishes` | Array[String] | No | Message types this handler publishes |
| `env` | Map[String, String] | No | Environment variables |
| `unwrap_stdout` | Boolean | No | When `true`, the SDK auto-extracts and parses the `.stdout` field from exec-source's `{command, stdout, exit_code}` envelope before delivering messages (engine sets `EMERGENT_UNWRAP_STDOUT=true` for the primitive) |

### [[sinks]]

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Unique identifier for this sink |
| `path` | Path | Yes | Path to executable |
| `args` | Array[String] | No | Command-line arguments |
| `enabled` | Boolean | No | Enable/disable (default: `true`) |
| `subscribes` | Array[String] | Yes | Message types to subscribe to |
| `env` | Map[String, String] | No | Environment variables |
| `unwrap_stdout` | Boolean | No | Same as for handlers — auto-unwrap exec-source's stdout envelope |

## Subscription Patterns

Subscriptions support wildcards:

| Pattern | Matches |
|---------|---------|
| `timer.tick` | Exact match only |
| `system.started.*` | `system.started.timer`, `system.started.filter`, etc. |
| `system.*` | `system.started`, `system.stopped`, etc. (one level) |

## Path Resolution

The engine resolves `path` in three ways:

1. **Tilde expansion**: `~/bin/app` → `/home/user/bin/app`
2. **Bare command lookup**: `path = "uv"` → searches PATH for `uv`
3. **"auto" for XDG paths**: Socket and event store paths support `"auto"`

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

## Multi-Language Paths

```toml
# Marketplace primitives (installed binaries)
path = "~/.local/share/emergent/primitives/bin/exec-handler"

# Rust (compiled binary)
path = "./target/release/my_handler"

# Python (via uv — recommended)
path = "uv"
args = ["run", "--with", "emergent-client", "my_handler.py"]

# TypeScript (via Deno)
path = "deno"
args = ["run", "--allow-env", "--allow-net=unix", "my_handler.ts"]

# Go (compiled binary)
path = "./my_handler"
```

## Secrets Management

Never hardcode tokens in TOML files. Use environment variables:

```toml
# Option 1: Environment variables (set before running)
[[sinks]]
name = "poster"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "data.post", "--", "sh", "-c", "curl -H \"Authorization: Bearer $API_TOKEN\" ..."]
subscribes = ["data.post"]

# Option 2: env field
[[sources]]
name = "webhook"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "8080"]
env = { HTTP_WEBHOOK_SECRET = "" }  # Set via export before running
publishes = ["http.request"]
```

For production:
- **Linux**: systemd-creds to encrypt secrets to TPM
- **macOS**: Keychain via `$(security find-generic-password -s KEY -w)` in args
