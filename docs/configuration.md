# Configuration Reference

Emergent uses TOML configuration files to declare pipeline topology. The engine reads the configuration at startup and enforces the declared message routing.

## File Location

```bash
# Specify the config file path when starting the engine
emergent --config /path/to/config.toml

# Common locations
emergent --config ./emergent.toml
emergent --config ~/.config/emergent/emergent.toml
```

## Complete Example

The `path` field for each primitive points to any executable: a compiled binary, an interpreter running your script, or a script with a shebang line. The engine spawns these as child processes.

```toml
# =============================================================================
# Engine Settings
# =============================================================================

[engine]
name = "emergent"
socket_path = "auto"
wire_format = "messagepack"
# api_port = 8891  # HTTP API port (0 to disable)

# =============================================================================
# Event Store Settings
# =============================================================================

[event_store]
json_log_dir = "./logs"
sqlite_path = "./events.db"
retention_days = 30

# =============================================================================
# Sources (publish only)
# =============================================================================

# Rust binary (compiled from your own project)
[[sources]]
name = "timer"
path = "/home/user/my-project/target/release/timer"
args = ["--interval", "5000"]
enabled = true
publishes = ["timer.tick"]

# =============================================================================
# Handlers (subscribe and publish)
# =============================================================================

# TypeScript handler (via Deno)
[[handlers]]
name = "filter"
path = "deno"
args = ["run", "--allow-env", "--allow-net=unix", "/home/user/my-project/filter.ts"]
enabled = true
subscribes = ["timer.tick"]
publishes = ["timer.filtered", "filter.processed"]

# =============================================================================
# Sinks (subscribe only)
# =============================================================================

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
wire_format = "messagepack"    # IPC wire format
api_port = 8891                # HTTP API port (0 to disable)
```

| Option | Default | Description |
|--------|---------|-------------|
| `name` | `"emergent"` | Engine instance name |
| `socket_path` | `"auto"` | `"auto"` for XDG-compliant path, or explicit path like `"/tmp/emergent.sock"` |
| `wire_format` | `"messagepack"` | `"messagepack"` (binary) or `"json"` (human-readable) |
| `api_port` | `8891` | HTTP API port for topology queries. Set to `0` to disable. |

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
| `json_log_dir` | `"./logs"` | Directory for append-only JSON log files |
| `sqlite_path` | `"./events.db"` | Path to SQLite database |
| `retention_days` | `30` | Days to retain events before cleanup |

The event store provides:
- **JSON logs**: Append-only files (one per day) for fast writes
- **SQLite**: Structured storage for queries and replay

## Sources

```toml
[[sources]]
name = "timer"
path = "/path/to/your/timer-binary"
args = ["--interval", "5000"]
enabled = true
publishes = ["timer.tick"]
```

| Option | Required | Description |
|--------|----------|-------------|
| `name` | Yes | Unique identifier |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments (array of strings) |
| `enabled` | No | `true` (default) or `false` to disable |
| `publishes` | Yes | Message types this source will emit |

Sources can only publish—they cannot subscribe.

## Handlers

```toml
[[handlers]]
name = "filter"
path = "/path/to/your/filter-binary"
args = ["--filter-every", "5"]
enabled = true
subscribes = ["timer.tick"]
publishes = ["timer.filtered"]
```

| Option | Required | Description |
|--------|----------|-------------|
| `name` | Yes | Unique identifier |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments |
| `enabled` | No | `true` (default) or `false` |
| `subscribes` | Yes | Message types to receive |
| `publishes` | Yes | Message types this handler will emit |

Handlers can both subscribe and publish.

## Sinks

```toml
[[sinks]]
name = "console"
path = "/path/to/your/console-binary"
args = ["--format", "json"]
enabled = true
subscribes = ["timer.filtered", "system.started.*"]
```

| Option | Required | Description |
|--------|----------|-------------|
| `name` | Yes | Unique identifier |
| `path` | Yes | Path to executable |
| `args` | No | Command-line arguments |
| `enabled` | No | `true` (default) or `false` |
| `subscribes` | Yes | Message types to receive |

Sinks can only subscribe—they cannot publish.

## Subscription Patterns

Subscriptions support wildcards:

```toml
subscribes = [
    "timer.tick",           # Exact match
    "system.started.*",     # Matches system.started.timer, system.started.filter, etc.
    "system.error.*",       # All error events
]
```

## Environment Variables

The engine sets these environment variables for each primitive:

| Variable | Description |
|----------|-------------|
| `EMERGENT_SOCKET` | Path to Unix socket for IPC |
| `EMERGENT_NAME` | Primitive's configured name |
| `EMERGENT_API_PORT` | HTTP API port for topology queries |
| `EMERGENT_PUBLISHES` | Comma-separated list of publish message types from config |
| `EMERGENT_SUBSCRIBES` | Comma-separated list of subscribe message types from config |

Primitives use these to connect:

```rust
let name = std::env::var("EMERGENT_NAME")?;
let source = EmergentSource::connect(&name).await?;
```

## Multi-Language Primitives

The engine spawns each primitive as a child process. The `path` field can point to a compiled binary or a language runtime that runs your script.

### Rust

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

# Or using uv for dependency management
[[sources]]
name = "webhook"
path = "uv"
args = ["run", "--project", "/home/user/my-project", "python", "/home/user/my-project/webhook.py"]
```

## Startup Order

The engine starts primitives in this order:

1. **Sinks** — started first so they are ready to receive messages
2. **Handlers** — started next so they can process messages
3. **Sources** — started last so they produce messages only when the pipeline is ready

Within each tier, primitives start in the order they appear in the configuration file. This ordering matters when one primitive depends on another's `system.started.*` event — the subscriber must appear before the publisher in the config.

At shutdown, the order reverses: Sources stop first (no new messages), then Handlers drain, then Sinks consume remaining messages.

## System Events

The engine publishes lifecycle events:

| Event | When |
|-------|------|
| `system.started.<name>` | Primitive connected |
| `system.stopped.<name>` | Primitive disconnected |
| `system.error.<name>` | Primitive failed |
| `system.shutdown` | Graceful shutdown signal |

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
path = "/path/to/your/enricher"
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

For unattended services on Linux, use `systemd-creds` to encrypt secrets to the machine's TPM or host key. Decrypted values are mounted at runtime and scoped to the service:

```ini
# /etc/systemd/system/emergent.service
[Service]
ExecStart=/usr/local/bin/emergent --config /etc/emergent/pipeline.toml
LoadCredentialEncrypted=SLACK_BOT_TOKEN:/etc/emergent/secrets/slack-bot-token.cred
LoadCredentialEncrypted=SLACK_APP_TOKEN:/etc/emergent/secrets/slack-app-token.cred
```

Primitives read the decrypted values from files at runtime:

```toml
args = ["--", "sh", "-c", "... -H \"Authorization: Bearer $(cat /run/credentials/emergent.service/SLACK_BOT_TOKEN)\" ..."]
```

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
- Paths must exist
- `subscribes` and `publishes` must be non-empty arrays
- Sources cannot have `subscribes`
- Sinks cannot have `publishes`
