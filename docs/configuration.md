# Configuration Reference

The TOML configuration file is your pipeline's architecture. It declares which processes run, what events they publish and subscribe to, and how they connect. The engine reads this file at startup and enforces the declared topology.

## File Location

```bash
emergent --config /path/to/config.toml
emergent --config ./emergent.toml
```

Use `emergent init` to generate a starter config interactively.

## Complete Example

This example shows all three primitive types: a marketplace exec-source running a shell command, a Deno-based TypeScript handler, and a Python sink. The `path` field for each primitive points to any executable -- the engine spawns these as child processes.

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

# Marketplace exec-source: run any shell command on an interval
[[sources]]
name = "timer"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "5000"]
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
| `wire_format` | `"messagepack"` | `"messagepack"` (binary) or `"json"` (human-readable, useful for debugging) |
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
| `json_log_dir` | `"./logs"` | Directory for append-only JSON log files (one per day) |
| `sqlite_path` | `"./events.db"` | Path to SQLite database for structured queries |
| `retention_days` | `30` | Days to retain events before cleanup |

Both paths support `"auto"` for XDG data directory placement.

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

## Subscription Patterns

Subscriptions support wildcards:

```toml
subscribes = [
    "timer.tick",           # Exact match
    "system.started.*",     # Matches system.started.timer, system.started.filter, etc.
    "system.error.*",       # All error events
]
```

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

At shutdown, the order reverses: Sources stop first (no new messages), then Handlers drain, then Sinks consume remaining messages.

## System Events

The engine publishes lifecycle events:

| Event | When |
|-------|------|
| `system.started.<name>` | Primitive connected |
| `system.stopped.<name>` | Primitive disconnected |
| `system.error.<name>` | Primitive failed |
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
- Paths must exist
- `subscribes` and `publishes` must be non-empty arrays
- Sources cannot have `subscribes`
- Sinks cannot have `publishes`
