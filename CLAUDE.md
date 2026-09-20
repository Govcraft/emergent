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

From 0.14.0 startup waits for each tier before starting the next: it
holds until every primitive in the tier that declares `subscribes` has reached
the engine over IPC, bounded by `[engine].startup_ready_timeout_ms` (default
5000). At the deadline the engine logs a WARN naming the primitives it never
heard from and carries on; a primitive that exits or fails during the wait
releases its tier; one with no `subscribes` is never waited on, so the source
tier does not wait. The decision is a pure function in
`emergent-engine/src/readiness.rs`. The signal is a subscribe the engine's IPC
policy authorized, reported through `PolicyObserver::on_subscribed`; until
issue #24 lands the connection is attributed to a primitive by matching the
peer's pid against the engine's own children, so a primitive connecting from a
grandchild process cannot be named and costs its tier the deadline. On 0.10.10 and earlier the
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
- `readiness.rs` — pure decision for "is this startup tier ready, and who is still missing", plus the policy observer that feeds it
- `declarations.rs` — pure decisions for declaration enforcement: modes, verdicts, the per-primitive topic table
- `api_host.rs`: pure decisions for which host the HTTP API answers to, plus the one axum middleware that applies them
- `ipc_policy.rs`: the `IpcSecurityPolicy` that holds each connection to those decisions
- `ipc_identity.rs`: `ConnectionIdentity` and the resolver admission asks who a peer is; the resolver that ships names nobody (issue #24)
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

- Wire format: MessagePack, always. `[engine].wire_format` is accepted so older configs keep loading but selects nothing; from 0.14.0 setting it warns at startup
- Transport: Unix domain sockets
- Messages registered with `#[acton_message(ipc)]` macro from acton-reactive
- Environment variables set by engine: `EMERGENT_SOCKET`, `EMERGENT_NAME`, `EMERGENT_PUBLISHES` (comma-separated), `EMERGENT_SUBSCRIBES` (comma-separated)

### HTTP API

- Axum-based server on configurable port (default: 8891, set `api_port = 0` to disable)
- From 0.14.0 it answers only to host names that are its own: an IP literal, `localhost`, or a name in `[engine].api_allowed_hosts`; anything else gets `421 Misdirected Request`
- `GET /api/topology` — returns all primitives with state, publishes, subscribes, PID

### Configuration

TOML-based configuration in `config/emergent.toml`:

- `[engine]` — `name`, `socket_path` ("auto" for XDG default), `api_port`, `api_allowed_hosts`, `max_connections`
- `[event_store]` — `json_log_dir`, `sqlite_path`, `retention_days` (paths support "auto" for XDG data dir)
- `[[sources]]` — `name`, `path`, `args`, `enabled`, `publishes`, `env`
- `[[handlers]]` / `[[sinks]]` — `name`, `path`, `args`, `enabled`, `subscribes`, `publishes`, `env`, `unwrap_stdout`

Path resolution: tilde expansion (`~/bin/app`), bare command lookup via PATH (`path = "uv"`), and "auto" XDG paths.

Unknown keys: from 0.14.0 every config table denies unknown fields, so a typo is a load error that names the key and its table. On 0.10.10 and earlier it was ignored.

Declaration enforcement: `[engine].enforce_declarations` is `"off"`, `"warn"` or `"strict"`, default `"off"`. From 0.14.0 it decides whether a primitive's `publishes` list binds it. `"warn"` logs a publish outside the declarations at WARN with the primitive, operation and message type; `"strict"` also refuses it, so the message is neither stored nor forwarded, the client gets an `ACCESS_DENIED` error carrying the engine's sentence, and the engine emits `system.error.<name>` with the reason. Matching is the same exact-or-trailing-wildcard rule as subscriptions. Protocol topics are always allowed on the operation they belong to: publishing `system.request.subscriptions` and `system.request.topology`, subscribing to `system.response.subscriptions`, `system.response.topology` and `system.shutdown`, all of which the SDKs do for you before your code runs. The engine's own `system.*` events arrive as `IpcSystemEvent` and never pass the check. On 0.10.10 and earlier the lists were advisory and nothing was checked. Enforcement runs in an `IpcSecurityPolicy` (acton-reactive 9.4.0), installed only when the mode is `warn` or `strict`, in `emergent-engine/src/ipc_policy.rs`; the decisions are pure functions in `emergent-engine/src/declarations.rs` and the lookup table is built once from config. The name a check is made under comes from the `source` field on the message, which the client writes itself, and the engine warns at startup that this is so: it catches every honest mistake and no lie, a refusal can be attributed to a primitive that did nothing wrong, and subscriptions are not checked at all because a subscribe frame carries no `source` and refusing on that basis would stop every handler and sink at startup. Resolving a connection to the primitive that opened it closes all three and is issue #24, which replaces `emergent-engine/src/ipc_identity.rs` and nothing else.

HTTP API host guard: the API has no authentication and binding `127.0.0.1` does not keep a browser out, because a page can re-resolve its own name to `127.0.0.1` and the browser then treats the API as that page's own origin. Up to engine 0.10.10 `curl -H 'Host: attacker.example' http://127.0.0.1:8891/api/topology` returned every primitive's name, kind, state, pid and topics. From 0.14.0 the API answers only when every host the request names is an IP literal, `localhost`, or a name in `[engine].api_allowed_hosts`, and returns `421 Misdirected Request` with a body naming that key otherwise. The port is never compared, so a port forward or a container opened as `localhost` is unaffected. Every `Host` header is checked rather than the first, and the request URI's authority as well, because hyper keeps a duplicate `Host` and an absolute-form request line intact, and `axum::serve` speaks HTTP/2 over cleartext where there is no `Host` header at all. A request naming no host is refused. The decisions are pure functions in `emergent-engine/src/api_host.rs`, whose table mirrors `primitives/sse-sink/host_test.ts` in emergent-primitives row for row so the two implementations cannot drift; `api_allowed_hosts` entries are validated at load, since a value with a scheme, port, path or `*` would be listed and never match.

Connection limit: every enabled primitive holds one IPC connection for the life of its process. The ceiling comes from acton-reactive, resolved from `$XDG_CONFIG_HOME/acton/ipc.toml` (`[limits] max_connections`) or its own default; `[engine].max_connections` overrides both, and leaving the key out keeps whatever acton resolved. From 0.14.0 the engine refuses to start when that limit cannot cover every enabled primitive plus `RESERVED_IPC_CONNECTIONS` (4: one for a restart overlap, three for CLI and topology-viewer queries), with an error naming both numbers. On 0.10.10 and earlier there was no check, so an oversized topology started with some primitives silently dropped at the accept semaphore while `/api/topology` still reported them running. The decision lives in `emergent-engine/src/config.rs` as `check_connection_capacity`, a pure function.

Marketplace: from 0.14.0 the registry is two files fetched over HTTPS,
`index.toml` and `manifests.toml`, published as assets of the emergent-primitives
release. `[marketplace].registry_url` (in `$XDG_CONFIG_HOME/emergent/marketplace.toml`)
is a base URL: one ending in `/releases` resolves to `latest/download/<file>` and
`download/v<version>/<file>`, which are redirects rather than API calls, so there
is no token and no rate limit; any other base is treated as a static host serving
`<file>` and `v<version>/<file>`. Both assets are cached under
`$XDG_CACHE_HOME/emergent/registry/<release>/`, a pinned release is read straight
from that cache, and an unreachable network falls back to it with a note. A `404`
is an answer, not an outage, and reports the URL it fetched. git is no longer
required. Engine 0.10.10 and earlier cloned `emergent-registry` instead and
installed a pinned version using the current manifest's filenames. URL
construction, checksum parsing and cache freshness live in
`emergent-engine/src/marketplace/registry.rs` as pure functions.

Retention: from 0.14.0 `retention_days` is enforced by a prune at startup and once a day, over both the SQLite store and the rotated `events-YYYY-MM-DD.jsonl` logs. `0` disables pruning. The decisions live in `emergent-engine/src/retention.rs` as pure functions.

## Release Process

Two repos are released, in this order: the SDKs, then emergent-primitives, then the engine. The primitives build against the published Rust SDK, and the engine's marketplace reads its catalog from the primitives release, so each step needs the one before it.

### Step 1: Release emergent (SDKs, then engine)

```bash
# 1. SDK version: bump the workspace version in Cargo.toml (emergent-client
#    inherits it), sdks/py/pyproject.toml, sdks/ts/deno.json and
#    sdks/ts/package.json. Two constants repeat it and each has a test that
#    fails when it drifts: `Version` in sdks/go/emergent.go and `__version__`
#    in sdks/py/emergent/__init__.py. Refresh sdks/py/uv.lock with `uv lock`.
# 2. Engine version: bump emergent-engine/Cargo.toml, and its emergent-client
#    dependency version with it. From 0.14.0 the engine and the SDKs share one
#    number; the field stays separate so either can take a patch release alone.
# 3. Update example deps to match (examples/*/Cargo.toml)

cargo fmt --all --check && cargo clippy --workspace --all-targets && cargo nextest run --workspace

# 4. Commit and push
git add -A && git commit -S -m "chore: bump SDKs to A.B.C and engine to X.Y.Z"
git push

# 5. Tag the SDKs. Each tag publishes one SDK (table below).
for sdk in rust py ts go; do git tag -s "sdks/$sdk/vA.B.C" -m "sdks/$sdk/vA.B.C"; done
git push origin sdks/rust/vA.B.C sdks/py/vA.B.C sdks/ts/vA.B.C sdks/go/vA.B.C

# 6. Release emergent-primitives (Step 2), then tag the engine
git tag -s vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z
```

The `vX.Y.Z` tag triggers the release workflow. It first runs `ci.yml` (the Rust, Python, TypeScript and Go gates) as its quality gate; the engine builds for Linux and macOS, the GitHub release, the `emergent-engine` crates.io publish and the AUR update all wait for it.

Each SDK publishes from its own tag, not from the engine tag:

| Tag | Workflow | Publishes |
|-----|----------|-----------|
| `sdks/rust/vX.Y.Z` | `workflow-crates-io.yml` | `emergent-client` to crates.io |
| `sdks/py/vX.Y.Z` | `workflow-pypi.yml` | Python SDK to PyPI |
| `sdks/ts/vX.Y.Z` | `workflow-jsr.yml` | `@govcraft/emergent` to JSR |
| `sdks/go/vX.Y.Z` | `workflow-go-proxy.yml` | Go module to the Go proxy |

Every SDK workflow also accepts a manual `workflow_dispatch`. The engine and the SDKs each carry their own version field, and from 0.14.0 they share one number (the engine went from 0.10.10 straight to 0.14.0 to line up with the SDKs). A `v0.11.0` engine tag and GitHub release exist from March 2026 and belong to no release line; leave them alone.

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

There is no third step. From primitives 0.12.0 each primitive's
`manifest.toml` lives next to its code, the release workflow generates
`index.toml` and `manifests.toml` from the manifests and the tag, and attaches
both to the release. From 0.14.0 the engine fetches those two assets
over HTTPS from `https://github.com/Govcraft/emergent-primitives/releases`, so
nothing has to be retyped in a third repository.

**The emergent-registry repo is archived, not deleted.** Engine 0.10.10 and
earlier have its git URL compiled in and clone it on every marketplace command,
so deleting it would break the marketplace for every engine already installed.
Its README points at the new location.

### Verification

```bash
emergent update                    # pulls latest engine binary
emergent marketplace update        # pulls latest primitive binaries
emergent marketplace list          # verify versions
```

## Linting Rules

Workspace-level clippy configuration denies `unwrap_used` and `expect_used`. Use proper error handling with `?` operator and Result types.

## Dependencies

- **acton-reactive**: Published crate (version 9.4.1) with features `ipc` and `ipc-messagepack` — provides the actor framework, IPC, message routing, and lifecycle management
- Uses Rust 2024 edition
- Release profile optimized for binary size: `opt-level = "z"`, LTO, single codegen unit, panic = abort, stripped
