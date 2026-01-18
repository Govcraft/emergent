# Configuration Reference

Emergent uses TOML configuration files to declare pipeline topology. The engine reads the configuration at startup and enforces the declared message routing.

## File Location

```bash
# Default location
./config/emergent.toml

# Or specify with --config
./target/release/emergent --config /path/to/config.toml
```

## Complete Example

```toml
# =============================================================================
# Engine Settings
# =============================================================================

[engine]
name = "emergent"
socket_path = "auto"
wire_format = "messagepack"

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

[[sources]]
name = "timer"
path = "./target/release/timer"
args = ["--interval", "5000"]
enabled = true
publishes = ["timer.tick"]

# =============================================================================
# Handlers (subscribe and publish)
# =============================================================================

[[handlers]]
name = "filter"
path = "./target/release/filter"
args = ["--filter-every", "5"]
enabled = true
subscribes = ["timer.tick"]
publishes = ["timer.filtered", "filter.processed"]

# =============================================================================
# Sinks (subscribe only)
# =============================================================================

[[sinks]]
name = "console"
path = "./target/release/console"
enabled = true
subscribes = ["timer.filtered", "filter.processed", "system.started.*"]
```

## Engine Section

```toml
[engine]
name = "emergent"              # Instance name (used in socket path)
socket_path = "auto"           # Socket location
wire_format = "messagepack"    # IPC wire format
```

| Option | Default | Description |
|--------|---------|-------------|
| `name` | `"emergent"` | Engine instance name |
| `socket_path` | `"auto"` | `"auto"` for XDG-compliant path, or explicit path like `"/tmp/emergent.sock"` |
| `wire_format` | `"messagepack"` | `"messagepack"` (binary) or `"json"` (human-readable) |

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
path = "./target/release/timer"
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
path = "./target/release/filter"
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
path = "./target/release/console"
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

Primitives use these to connect:

```rust
let name = std::env::var("EMERGENT_NAME")?;
let source = EmergentSource::connect(&name).await?;
```

## Multi-Language Primitives

### Rust

```toml
[[handlers]]
name = "filter"
path = "./target/release/filter"
```

### TypeScript (Deno)

```toml
[[sinks]]
name = "console_color"
path = "/path/to/deno"
args = [
    "run",
    "--allow-env",
    "--allow-read",
    "--allow-write",
    "--allow-net=unix",
    "./examples/sinks/console_color/main.ts"
]
```

### Python (uv)

```toml
[[sources]]
name = "webhook"
path = "/usr/bin/uv"
args = ["run", "--project", "./examples/sources/webhook", "python", "./main.py"]
```

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
path = "./target/release/enricher"
enabled = false  # Temporarily disabled
subscribes = ["event.raw"]
publishes = ["event.enriched"]
```

## Validation

The engine validates configuration at startup:

- All primitive names must be unique
- Paths must exist
- `subscribes` and `publishes` must be non-empty arrays
- Sources cannot have `subscribes`
- Sinks cannot have `publishes`
