# Examples

Emergent ships with two sets of examples: **zero-code pipelines** that run entirely from marketplace primitives and TOML configuration, and **advanced examples** that demonstrate fan-in, fan-out, stateful transformation, and real-time browser visualization.

---

## Zero-Code Pipelines

These pipelines require no application code. Install the marketplace primitives, point the engine at a TOML file, and run.

### Prerequisites

```bash
emergent marketplace install exec-source exec-handler exec-sink \
  http-source websocket-handler topology-viewer
```

### basic-pipeline.toml

Runs `date` every 3 seconds, pipes through `jq` to add a field, pretty-prints the result. Includes the topology viewer on port 8009 for a live visualization of the dataflow.

```
exec-source (date) ──> exec-handler (jq transform) ──> exec-sink (pretty-print)
                                                    └─> topology-viewer (port 8009)
```

```bash
emergent --config ./config/examples/basic-pipeline.toml
# Open http://localhost:8009 to see the topology
```

### ouroboros-loop.toml

A self-seeding infinite loop. The loopback sink subscribes to `system.started.webhook` to seed the first iteration, then `loop.iteration` keeps it going. The counter increments on every pass.

```
http-source ──> exec-handler (jq: increment counter) ──> exec-sink (printer)
     ^                                                └─> exec-sink (loopback curl)
     └────────────────────────────────────────────────────┘
```

This demonstrates how system events (`system.started.*`) can trigger application logic — the loop bootstraps itself without any manual `curl`.

```bash
emergent --config ./config/examples/ouroboros-loop.toml
```

### websocket-echo.toml

Connects to a WebSocket echo server, sends a test message, prints the echoed response. Demonstrates the websocket-handler's bidirectional bridge — subscribe to send outbound frames, publish inbound frames as events.

```
exec-source (URL) ──> websocket-handler ──> exec-sink (print frames)
                           ^       |
                           |       └─> exec-handler (send test message on connect)
                           └───────────┘
```

```bash
emergent --config ./config/examples/websocket-echo.toml
```

### slack-bot.toml

A Claude-powered Slack chatbot using Socket Mode (no public URL needed). The pipeline:

1. Calls the Slack API to get a Socket Mode WebSocket URL
2. Connects via websocket-handler
3. Auto-acks envelopes within Slack's 3-second window
4. Extracts user messages (filtering bot messages, acks, hellos)
5. Sends message text to Claude
6. Posts the response back to the channel

All eight primitives are marketplace exec-handlers and exec-sinks chained with `jq` — no custom code.

```bash
export SLACK_APP_TOKEN="xapp-..."
export SLACK_BOT_TOKEN="xoxb-..."
emergent --config ./config/examples/slack-bot.toml
```

Requires a Slack app configured with Socket Mode, `chat:write`, and message event subscriptions. See the comments in the [TOML file](../config/examples/slack-bot.toml) for full setup steps.

---

## Advanced Examples

These pipelines demonstrate event-driven patterns beyond linear chains. Each includes a custom handler (Python or Rust script) alongside marketplace primitives.

### Prerequisites

```bash
emergent marketplace install exec-source exec-sink sse-sink
```

### system-monitor

Six exec-sources poll system metrics (CPU, memory, load, disk usage, disk I/O, network I/O) on independent intervals. All converge on a stateful Python handler that computes per-second rates from cumulative counters, then fan out to a live SSE dashboard and console output.

```
exec-source (cpu)     ──┐
exec-source (memory)  ──┤
exec-source (load)    ──┤
exec-source (disk)    ──┼─> Python handler (stateful deltas) ──┬─> sse-sink (browser dashboard)
exec-source (disk-io) ──┤                                      └─> exec-sink (console)
exec-source (net-io)  ──┘
```

```bash
emergent --config ./config/advanced-examples/system-monitor/emergent.toml
# Open http://localhost:8080 to see live metrics with sparkline charts
```

![System Monitor Dashboard](images/system-monitor.png)

**What this demonstrates:**

- **Fan-in**: Six independent sources converge on one handler. No routing logic — they all publish metric types, the handler subscribes to all of them.
- **Fan-out**: One handler's output goes to multiple sinks simultaneously. Add a third sink (Slack alerts, file logging) without modifying any existing primitive.
- **Stateful handler**: The Python handler maintains state across messages to compute per-second deltas from cumulative OS counters (CPU, disk I/O, network I/O).
- **Mixed primitives**: Marketplace exec-sources for data collection, a custom Python handler for domain logic, marketplace sinks for output.
- **Independent intervals**: Each source polls at its own rate. No orchestrator coordinating timing.
- **Decoupled extensibility**: Add a new metric source by adding one `[[sources]]` block. The handler and sinks don't change.

### game-of-life

Conway's Game of Life as an event-driven pipeline. A one-shot source seeds the grid, a timer drives generations, and a stateful Python handler evolves the world. Complex patterns — gliders, oscillators, spaceships — emerge from four simple rules applied to a pub-sub message stream.

```
exec-source (seed, one-shot) ──> Python handler (world state + rules) ──┬─> sse-sink (canvas)
exec-source (clock, 150ms)   ──┘                                        └─> exec-sink (console)
```

```bash
emergent --config ./config/advanced-examples/game-of-life/emergent.toml
# Open http://localhost:8082 to watch
```

![Game of Life](images/game-of-life.png)

Switch patterns with an environment variable:

```bash
LIFE_PATTERN=gosper-gun emergent --config ./config/advanced-examples/game-of-life/emergent.toml
```

Available patterns: `glider`, `blinker`, `pulsar`, `r-pentomino`, `acorn`, `gosper-gun`.

**What this demonstrates:**

- **Stateful transformation**: The Python handler maintains a full grid across messages, applying Game of Life rules on each tick.
- **One-shot seeding**: The seed source fires once and exits. The timer continues driving the simulation indefinitely.
- **Emergent complexity**: Four simple rules, one handler, two sinks. Gliders navigate, oscillators pulse, spaceships fly — all from a single seed event flowing through pub-sub.
- **Real-time streaming**: The SSE sink pushes every generation to the browser. The canvas renders incrementally (only changed cells).
- **Environment-driven configuration**: `LIFE_PATTERN` selects the seed pattern without changing any config or code.

### reaction-diffusion

The Gray-Scott model: two chemicals diffuse and react on a grid, creating Turing patterns — the same math behind animal skin markings. A Rust script handler (using `cargo -Zscript`, no build step) computes the simulation with rayon parallelism across CPU cores, publishing each frame via SSE to a browser canvas with a heat-map palette.

```
exec-source (seed, one-shot) ──> Rust handler (Gray-Scott + rayon) ──> sse-sink (canvas)
exec-source (clock, 30ms)    ──┘
```

```bash
emergent --config ./config/advanced-examples/reaction-diffusion/emergent.toml
# Open http://localhost:8084 to watch patterns emerge
```

![Reaction-Diffusion](images/reaction-diffusion.png)

Switch presets:

```bash
RD_PRESET=maze emergent --config ./config/advanced-examples/reaction-diffusion/emergent.toml
```

Available presets: `mitosis`, `coral`, `maze`, `holes`, `ripple`, `spots`, `worms`.

Requires Rust nightly for `cargo -Zscript` (single-file scripts with inline dependencies).

**What this demonstrates:**

- **Rust script handler**: A single `.rs` file with inline `Cargo.toml` dependencies, run via `cargo +nightly -Zscript`. No workspace entry, no build step — just a script next to the config.
- **Parallel computation**: Rayon parallelizes the simulation across CPU cores. Each row of the 100x140 grid is computed independently, enabling 16 substeps per 30ms frame.
- **Multi-SDK showcase**: The three advanced examples use three different handler approaches — marketplace exec-handler (jq), Python SDK (`run_handler`), and Rust script. Same engine, same pub-sub protocol, best tool for each job.
- **Emergent complexity**: Two chemicals, four parameters. Spots, stripes, coral, and labyrinths self-organize from uniform initial conditions.

---

## Secrets

Never hardcode tokens in TOML. Use environment variables — the engine forwards the parent process environment to all primitives. Set secrets via `export` or `source .env` before running.

For production deployments, see [Configuration](configuration.md#secrets) for systemd-creds (Linux) and Keychain (macOS) patterns.

---

## See Also

- [Getting Started](getting-started.md) — Build your first pipeline from scratch
- [Configuration](configuration.md) — All configuration options
- [Concepts](concepts.md) — Architecture, message flow, event sourcing
