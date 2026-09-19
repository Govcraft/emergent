# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Development Commands

```bash
# Build all workspace members
cargo build --release

# Check code (faster than build)
cargo check

# Run tests
cargo nextest run

# Run clippy lints
cargo clippy --all-targets

# Run the engine with example config
./target/release/emergent --config ./config/emergent.toml

# Run a single example primitive (for testing)
cargo run --release -p timer -- --interval 5000
cargo run --release -p filter -- --filter-every 5
cargo run --release -p console
cargo run --release -p log -- --output ./timer_events.log
cargo run --release -p exec

# Scaffold a new primitive (interactive or scripted)
emergent scaffold
emergent scaffold -t handler -n my_filter -l rust -S timer.tick -p timer.filtered

# Initialize a new config file
emergent init

# Marketplace commands
emergent marketplace list
emergent marketplace install http-source
```

## Architecture Overview

Emergent is an **event-driven workflow engine** built on **acton-reactive** (a Rust actor framework). It implements a publish-subscribe pattern using three primitive types that communicate via Unix IPC sockets.

### Core Components

```
┌──────────────────────────────────────────────────────────────────────┐
│                        Emergent Engine                               │
│  ┌─────────────┐  ┌─────────────┐  ┌───────────────┐  ┌──────────┐  │
│  │  Process    │  │    IPC      │  │  Event Store  │  │ HTTP API │  │
│  │  Manager    │  │   Server    │  │ (JSON+SQLite) │  │ (Axum)   │  │
│  └─────────────┘  └─────────────┘  └───────────────┘  └──────────┘  │
└──────────────────────────────────────────────────────────────────────┘
        │                 │                                   │
        ▼                 ▼                                   ▼
   ┌─────────┐      ┌───────────┐      ┌────────┐     /api/topology
   │ Sources │      │ Handlers  │      │ Sinks  │
   └─────────┘      └───────────┘      └────────┘
```

### Startup & Shutdown Order

- **Startup**: Sinks → Handlers → Sources (consumers ready before producers)
- **Shutdown**: Sources (SIGTERM) → Handlers (`system.shutdown` broadcast) → Sinks (`system.shutdown` broadcast)

After engine 0.10.10 startup waits for each tier before starting the next: it
holds until every primitive in the tier that declares `subscribes` has reached
the engine over IPC, bounded by `[engine].startup_ready_timeout_ms` (default
5000). At the deadline the engine logs a WARN naming the primitives it never
heard from and carries on; a primitive that exits or fails during the wait
releases its tier; one with no `subscribes` is never waited on, so the source
tier does not wait. The decision is a pure function in
`emergent-engine/src/readiness.rs`, which also documents what the engine can
and cannot observe about a primitive's subscription. On 0.10.10 and earlier the
engine slept a fixed 50 ms per primitive and started the next tier regardless,
so a slow-starting subscriber missed the first events (Govcraft/emergent#66).

### The Three Primitives

| Primitive | Capabilities | Purpose |
|-----------|-------------|---------|
| **Source** | Publish only | Ingress - emit events into the system |
| **Handler** | Subscribe + Publish | Transform - process and re-emit events |
| **Sink** | Subscribe only | Egress - consume events (logs, console, HTTP) |

### Workspace Structure

- **emergent-engine** (`emergent-engine/`): Core runtime, process manager, message broker, event store, scaffold, marketplace
- **emergent-client** (`sdks/rust/`): Rust SDK for building Sources, Handlers, and Sinks
- **sdks/ts**: TypeScript/Deno SDK
- **sdks/py**: Python SDK (uses `uv` for package management)
- **sdks/go**: Go SDK
- **examples/sources/**: timer (Rust), timer-go (Go), webhook (Python), topology-api (TypeScript)
- **examples/handlers/**: filter (Rust), filter-go (Go), exec (Rust)
- **examples/sinks/**: console (Rust), console-go (Go), log (Rust), console_color (TypeScript), webhook_console (Python), topology-viewer (TypeScript)

### Engine Modules

- `config.rs` — TOML config loading, path expansion, validation
- `process_manager.rs` — Actor-based lifecycle for primitives
- `primitive_actor.rs` — Per-primitive actor (spawns child process, monitors, broadcasts system events, owns the primitive's live state)
- `lifecycle.rs`: pure state machine mapping a lifecycle event to a primitive's next state, pid and error
- `readiness.rs` — pure decision for "is this startup tier ready, and who is still missing", plus the acton probe that feeds it
- `event_store/` — JSON append-only logs + SQLite structured storage
- `scaffold/` — Code generation for new primitives (Rust, Python, TypeScript templates)
- `marketplace/` — Registry client for discovering and installing community primitives
- `init/` — Interactive `emergent init` to create emergent.toml

### Key Abstractions

**EmergentMessage** (`sdks/rust/src/message.rs`) - The universal message envelope:
```rust
pub struct EmergentMessage {
    pub id: MessageId,                    // TypeID: msg_<UUIDv7>
    pub message_type: MessageType,        // e.g., "timer.tick"
    pub source: PrimitiveName,            // primitive name
    pub correlation_id: Option<CorrelationId>,
    pub causation_id: Option<CausationId>,  // enables event tracing
    pub timestamp_ms: Timestamp,          // Unix ms
    pub payload: serde_json::Value,
    pub metadata: Option<serde_json::Value>,
}
```
Types are in `sdks/rust/src/types/` — `MessageId`, `MessageType`, `PrimitiveName`, `CorrelationId`, `CausationId`, `Timestamp`.

**System Events** — Engine broadcasts lifecycle events:
- `system.started.<name>` - primitive started successfully
- `system.stopped.<name>` - primitive stopped gracefully
- `system.error.<name>` - primitive failed
- `system.shutdown` - signals primitives to gracefully stop
- `system.request.subscriptions` / `system.response.subscriptions` - SDK subscription discovery
- `system.request.topology` / `system.response.topology` - topology queries via pub/sub

### Subscription Matching

A subscription is an exact message type or a prefix ending in a single trailing
`*` (`system.error.*`, or `*` for everything). Matching lives in acton's
`subscribe_patterns` API, not in an engine-side table, because the engine
already forwards each message under its Emergent `message_type` string.
Mid-string wildcards such as `system.*.error` are rejected by config validation
and by the SDKs. Overlapping topics deliver one copy per message. Engine
0.10.10 and earlier accepted wildcard subscriptions and never delivered them.

### IPC Protocol

- Wire format: MessagePack, always. `[engine].wire_format` is accepted so older configs keep loading but selects nothing; after engine 0.10.10 setting it warns at startup
- Transport: Unix domain sockets
- Messages registered with `#[acton_message(ipc)]` macro from acton-reactive
- Environment variables set by engine: `EMERGENT_SOCKET`, `EMERGENT_NAME`, `EMERGENT_PUBLISHES` (comma-separated), `EMERGENT_SUBSCRIBES` (comma-separated)

### HTTP API

- Axum-based server on configurable port (default: 8891, set `api_port = 0` to disable)
- `GET /api/topology` — returns all primitives with state, publishes, subscribes, PID

### Configuration

TOML-based configuration in `config/emergent.toml`:

- `[engine]` — `name`, `socket_path` ("auto" for XDG default), `api_port`, `max_connections`
- `[event_store]` — `json_log_dir`, `sqlite_path`, `retention_days` (paths support "auto" for XDG data dir)
- `[[sources]]` — `name`, `path`, `args`, `enabled`, `publishes`, `env`
- `[[handlers]]` / `[[sinks]]` — `name`, `path`, `args`, `enabled`, `subscribes`, `publishes`, `env`, `unwrap_stdout`

Path resolution: tilde expansion (`~/bin/app`), bare command lookup via PATH (`path = "uv"`), and "auto" XDG paths.

Unknown keys: after engine 0.10.10 every config table denies unknown fields, so a typo is a load error that names the key and its table. On 0.10.10 and earlier it was ignored.

Connection limit: every enabled primitive holds one IPC connection for the life of its process. The ceiling comes from acton-reactive, resolved from `$XDG_CONFIG_HOME/acton/ipc.toml` (`[limits] max_connections`) or its own default; `[engine].max_connections` overrides both, and leaving the key out keeps whatever acton resolved. After engine 0.10.10 the engine refuses to start when that limit cannot cover every enabled primitive plus `RESERVED_IPC_CONNECTIONS` (4: one for a restart overlap, three for CLI and topology-viewer queries), with an error naming both numbers. On 0.10.10 and earlier there was no check, so an oversized topology started with some primitives silently dropped at the accept semaphore while `/api/topology` still reported them running. The decision lives in `emergent-engine/src/config.rs` as `check_connection_capacity`, a pure function.

Retention: after engine 0.10.10 `retention_days` is enforced by a prune at startup and once a day, over both the SQLite store and the rotated `events-YYYY-MM-DD.jsonl` logs. `0` disables pruning. The decisions live in `emergent-engine/src/retention.rs` as pure functions.

## Release Process

Three repos must be released in order. The Rust SDK must be published to crates.io before primitives can build against it.

### Step 1: Release emergent (engine + SDKs)

```bash
# 1. Bump workspace version in Cargo.toml and emergent-engine/Cargo.toml
# 2. Update example deps to match (examples/*/Cargo.toml)
# 3. Bump Python SDK version in sdks/py/pyproject.toml
# 4. Bump TypeScript SDK version in sdks/ts/deno.json and sdks/ts/package.json

cargo check && cargo clippy --all-targets && cargo nextest run

# 5. Publish Rust SDK to crates.io (must happen before primitives build)
cd sdks/rust && cargo publish

# 6. Commit, push, tag
git add -A && git commit -S -m "chore: bump to X.Y.Z"
git push && git tag -s vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z
```

The `vX.Y.Z` tag triggers the release workflow. It first runs `ci.yml` (the Rust, Python, TypeScript and Go gates) as its quality gate; the engine builds for Linux and macOS, the GitHub release, the `emergent-engine` crates.io publish and the AUR update all wait for it.

Each SDK publishes from its own tag, not from the engine tag:

| Tag | Workflow | Publishes |
|-----|----------|-----------|
| `sdks/rust/vX.Y.Z` | `workflow-crates-io.yml` | `emergent-client` to crates.io |
| `sdks/py/vX.Y.Z` | `workflow-pypi.yml` | Python SDK to PyPI |
| `sdks/ts/vX.Y.Z` | `workflow-jsr.yml` | `@govcraft/emergent` to JSR |
| `sdks/go/vX.Y.Z` | `workflow-go-proxy.yml` | Go module to the Go proxy |

Every SDK workflow also accepts a manual `workflow_dispatch`. The engine and the SDKs are versioned separately (engine 0.10.x, SDKs 0.13.x at the time of writing). A `v0.11.0` engine tag and GitHub release already exist from March 2026, so the next engine minor must skip that number.

### Step 2: Release emergent-primitives

```bash
# 1. Bump workspace version in Cargo.toml
# 2. Update emergent-client dependency version in Cargo.toml
# 3. Update JSR import versions in Deno primitives (jsr:@govcraft/emergent@X.Y.Z)

cd /path/to/emergent-primitives
cargo check && cargo clippy --all-targets && cargo nextest run
deno check primitives/topology-viewer/main.ts
deno check primitives/websocket-handler/main.ts
deno check primitives/sse-sink/main.ts

# 4. Commit, push, tag
git add -A && git commit -S -m "chore: bump to X.Y.Z"
git push && git tag -s vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z
```

Tagging triggers the release workflow which builds Rust + Deno binaries for all platforms.

### Step 3: Update emergent-registry

```bash
# 1. Update version in index.toml and all primitives/*/manifest.toml
cd /path/to/emergent-registry
sed -i 's/OLD_VERSION/NEW_VERSION/g' index.toml primitives/*/manifest.toml

# 2. If a new primitive was added, create its manifest directory and manifest.toml

# 3. Commit and push
git add -A && git commit -S -m "chore: bump to X.Y.Z"
git push
```

No tagging needed — the registry is a plain git repo that the engine clones/pulls.

### Verification

```bash
emergent update                    # pulls latest engine binary
emergent marketplace update        # pulls latest primitive binaries
emergent marketplace list          # verify versions
```

## Linting Rules

Workspace-level clippy configuration denies `unwrap_used` and `expect_used`. Use proper error handling with `?` operator and Result types.

## Dependencies

- **acton-reactive**: Published crate (version 9.4.0) with features `ipc` and `ipc-messagepack` — provides the actor framework, IPC, message routing, and lifecycle management
- Uses Rust 2024 edition
- Release profile optimized for binary size: `opt-level = "z"`, LTO, single codegen unit, panic = abort, stripped
