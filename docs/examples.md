# Examples

Emergent ships with two sets of examples: **zero-code pipelines** built entirely from marketplace primitives and TOML configuration, and **advanced examples** demonstrating fan-in, fan-out, stateful handlers, and real-time browser visualization.

---

## Zero-Code Pipelines

These pipelines require no application code. Install the marketplace primitives, point the engine at a TOML file, and run.

### Prerequisites

```bash
emergent marketplace install exec-source exec-handler exec-sink \
  http-source websocket-handler topology-viewer
```

---

### slack-bot

An 8-step Claude-powered Slack chatbot using Socket Mode (no public URL needed). This is the flagship example of tool-agnostic AI orchestration: the entire chatbot -- WebSocket connectivity, message filtering, LLM processing, Slack API calls -- is pure TOML composition with jq, curl, and one model call.

**Pipeline:**

```
slack-connect ──> url-extractor ──> slack-ws ──> slack-ack (auto-ack envelopes)
                                       │
                                       └──> slack-extract ──> prepare-prompt ──> claude-respond ──> slack-poster
```

**What each step does:**

1. **slack-connect** (source): Calls the Slack API to get a Socket Mode WebSocket URL
2. **url-extractor** (handler): Unwraps the URL from the exec-source stdout wrapper
3. **slack-ws** (handler): Connects to Slack via WebSocket, publishes inbound frames
4. **slack-ack** (handler): Auto-acknowledges Slack envelopes within the 3-second window
5. **slack-extract** (handler): Filters for user messages (ignores bot messages, acks, hellos)
6. **prepare-prompt** (handler): Extracts channel + text for the LLM
7. **claude-respond** (handler): Sends message text to Claude, returns channel + response
8. **slack-poster** (sink): Posts the LLM's response back to the Slack channel

**The model is interchangeable.** Step 7 calls `claude -p`, but you can substitute any CLI tool:
- `curl` to Ollama for local inference
- `curl` to OpenAI, Anthropic, or any HTTP API
- `python3 classify.py` for a scikit-learn classifier
- Any executable that reads stdin and writes stdout

**Running it:**

```bash
export SLACK_APP_TOKEN="xapp-..."
export SLACK_BOT_TOKEN="xoxb-..."
emergent --config ./config/examples/slack-bot.toml
```

**Slack App Setup:**

1. Create an app at https://api.slack.com/apps
2. Enable Socket Mode and create an App-Level Token with `connections:write`
3. Under OAuth & Permissions, add scopes: `chat:write`, `channels:history`, `im:history`
4. Under Event Subscriptions, subscribe to: `message.channels`, `message.im`
5. Install to your workspace

Never hardcode tokens in TOML. See [Configuration > Secrets](configuration.md#secrets) for production patterns (systemd-creds, macOS Keychain).

[View the full config](../config/examples/slack-bot.toml)

---

### basic-pipeline

The simplest possible pipeline. Runs `date` every 3 seconds, pipes through jq to transform, pretty-prints the result. Includes the topology viewer for a live visualization of the dataflow.

```
exec-source (date) ──> exec-handler (jq transform) ──> exec-sink (pretty-print)
                                                    └─> topology-viewer (port 8009)
```

```bash
emergent --config ./config/examples/basic-pipeline.toml
# Open http://localhost:8009 to see the topology
```

**What this demonstrates:** The three-primitive model in its simplest form. Source produces events, Handler transforms them, Sink consumes them. Every more complex pipeline follows this same pattern.

---

### ouroboros-loop

A self-seeding infinite loop. An exec-sink subscribes to both `system.started.webhook` (to seed the first iteration) and `loop.iteration` (to keep it going), curling back to the http-source on each pass. The counter increments every cycle.

```
http-source ──> exec-handler (jq: increment counter) ──> exec-sink (printer)
     ^                                                └─> exec-sink (loopback curl)
     └────────────────────────────────────────────────────┘
```

```bash
emergent --config ./config/examples/ouroboros-loop.toml
```

**What this demonstrates:** System events as application triggers. The loop bootstraps itself from `system.started.webhook` -- no manual curl needed. This pattern is useful for self-initializing pipelines and health-check loops.

---

### websocket-echo

Connects to a WebSocket echo server, sends a test message, prints the echoed response. Demonstrates the websocket-handler's bidirectional bridge: subscribe to `ws.send` to transmit outbound frames, receive inbound frames as `ws.frame` events.

```
exec-source (URL) ──> url-extractor ──> websocket-handler ──> exec-sink (print frames)
                                            ^       |
                                            |       └─> exec-handler (send test message on connect)
                                            └───────────┘
```

```bash
emergent --config ./config/examples/websocket-echo.toml
```

**What this demonstrates:** Bidirectional real-time communication. The WebSocket handler bridges the pub-sub world with the WebSocket world. This is the same pattern the slack-bot uses for Socket Mode.

---

## Advanced Examples

These pipelines demonstrate event-driven patterns beyond linear chains. Each includes a custom handler (Python or Rust script) alongside marketplace primitives.

### Prerequisites

```bash
emergent marketplace install exec-source exec-sink sse-sink
```

---

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

**Patterns demonstrated:**

- **Fan-in**: Six independent sources converge on one handler. No routing logic needed -- they all publish metric types, the handler subscribes to all of them.
- **Fan-out**: One handler's output goes to multiple sinks simultaneously. Add a third sink (Slack alerts, file logging) by adding one `[[sinks]]` block.
- **Stateful handler**: The Python handler maintains state across messages to compute per-second deltas from cumulative OS counters. This is why it uses a custom SDK primitive instead of a stateless exec-handler.
- **Independent intervals**: Each source polls at its own rate. No orchestrator coordinating timing.
- **Decoupled extensibility**: Add a new metric source by adding one `[[sources]]` block. The handler and sinks do not change.

---

### game-of-life

Conway's Game of Life as an event-driven pipeline. A one-shot source seeds the grid, a timer drives generations, and a stateful Python handler evolves the world. Complex patterns -- gliders, oscillators, spaceships -- emerge from four simple rules applied to a pub-sub message stream.

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

**Patterns demonstrated:**

- **Stateful transformation**: The Python handler maintains a full grid across messages, applying Game of Life rules on each tick.
- **One-shot seeding**: The seed source fires once and exits. The timer continues driving the simulation.
- **Real-time streaming**: The SSE sink pushes every generation to the browser. The canvas renders incrementally.
- **Environment-driven configuration**: `LIFE_PATTERN` selects the seed pattern without changing config or code.

---

### reaction-diffusion

The Gray-Scott model: two chemicals diffuse and react on a grid, creating Turing patterns -- the same math behind animal skin markings. A Rust script handler (using `cargo -Zscript`, no build step) computes the simulation with rayon parallelism across CPU cores, publishing each frame via SSE to a browser canvas.

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

**Patterns demonstrated:**

- **Rust script handler**: A single `.rs` file with inline `Cargo.toml` dependencies, run via `cargo +nightly -Zscript`. No workspace entry, no build step.
- **Parallel computation**: Rayon parallelizes the simulation across CPU cores. Each row of the 100x140 grid is computed independently.
- **Multi-SDK showcase**: The three advanced examples use three different handler approaches -- marketplace exec-handler (jq), Python SDK, and Rust script. Same engine, same protocol, best tool for each job.

---

## Composing Your Own Pipelines

The examples above demonstrate patterns you can combine:

| Pattern | How to Apply |
|---------|-------------|
| **Linear chain** | Source -> Handler -> Sink (basic-pipeline) |
| **Fan-in** | Multiple sources -> one handler (system-monitor) |
| **Fan-out** | One handler -> multiple sinks (system-monitor) |
| **Self-seeding loop** | System events trigger application logic (ouroboros-loop) |
| **Bidirectional bridge** | WebSocket/SSE handler bridges protocols (slack-bot) |
| **Stateful handler** | Custom SDK primitive maintains state (game-of-life) |
| **Tool-agnostic model call** | exec-handler wraps any CLI tool (slack-bot) |

Start with exec primitives. When you need persistent state, custom protocols, or high-performance processing, reach for a custom SDK primitive. The mental model is the same either way: Sources publish, Handlers transform, Sinks consume.

---

## Secrets

Never hardcode tokens in TOML. Use environment variables -- the engine forwards the parent process environment to all primitives:

```bash
export SLACK_APP_TOKEN="xapp-..."
export SLACK_BOT_TOKEN="xoxb-..."
emergent --config ./emergent.toml
```

For production deployments, see [Configuration > Secrets](configuration.md#secrets) for systemd-creds (Linux) and Keychain (macOS) patterns.

---

## See Also

- [Getting Started](getting-started.md) -- Install, run your first pipeline, extend it
- [Concepts](concepts.md) -- Architecture, message flow, event sourcing
- [Configuration](configuration.md) -- All configuration options
