# Advanced Examples

Pipelines that demonstrate event-driven patterns beyond linear chains: fan-in, fan-out, stateful transformation, and real-time browser integration.

## system-monitor

Six exec-sources poll system metrics (CPU, memory, load, disk usage, disk I/O, network I/O) on independent intervals. All converge on a stateful Python handler that computes per-second rates from cumulative counters (fan-in), then fans out to a live SSE dashboard and console output.

```
exec-source (cpu)     ──┐
exec-source (memory)  ──┤
exec-source (load)    ──┤
exec-source (disk)    ──┼→ Python handler (stateful deltas) ──┬→ sse-sink (browser dashboard)
exec-source (disk-io) ──┤                                     └→ exec-sink (console)
exec-source (net-io)  ──┘
```

### Prerequisites

```bash
emergent marketplace install exec-source exec-sink sse-sink
```

### Usage

Run from the repo root:

```bash
emergent --config ./config/advanced-examples/system-monitor/emergent.toml
```

The pipeline auto-starts a web server on port 8080. Open http://localhost:8080 to see live metrics with sparkline charts. The SSE sink pushes updates on port 8081.

To run from a different directory, set the dashboard path:

```bash
export EMERGENT_DASHBOARD_DIR=/path/to/emergent/config/advanced-examples/system-monitor
emergent --config /path/to/emergent/config/advanced-examples/system-monitor/emergent.toml
```

### What this demonstrates

- **Fan-in**: Six independent sources converge on one handler. No routing logic — they all publish metric types, the handler subscribes to all of them.
- **Fan-out**: One handler's output goes to multiple sinks simultaneously. Add a third sink (Slack alerts, file logging) without modifying any existing primitive.
- **Stateful handler**: The Python handler (`metric_formatter.py`) maintains state across messages to compute per-second deltas from cumulative OS counters (CPU, disk I/O, network I/O). This is the pattern for writing custom handlers with any SDK — contrast with stateless marketplace primitives like exec-handler.
- **Mixed primitives**: Marketplace exec-sources for data collection, a custom Python handler for domain logic, marketplace sinks for output. Each piece uses the best tool for the job.
- **Independent intervals**: Each source polls at its own rate. No orchestrator coordinating timing.
- **Decoupled extensibility**: Add a new metric source by adding one `[[sources]]` block. The handler and sinks don't change.

## game-of-life

Conway's Game of Life as an event-driven pipeline. A one-shot source seeds the grid, a timer drives generations, and a stateful Python handler evolves the world. Complex patterns — gliders, oscillators, spaceships — emerge from four simple rules applied to a pub-sub message stream.

```
exec-source (seed, one-shot) ──→ Python handler (world state + rules) ──┬→ sse-sink (canvas)
exec-source (clock, 150ms)   ──┘                                        └→ exec-sink (console)
```

### Prerequisites

```bash
emergent marketplace install exec-source exec-sink sse-sink
```

### Usage

Run from the repo root:

```bash
emergent --config ./config/advanced-examples/game-of-life/emergent.toml
```

Open http://localhost:8082 to watch. The default pattern (r-pentomino) evolves for ~1100 generations before stabilizing into gliders and oscillators.

Switch patterns with an environment variable:

```bash
LIFE_PATTERN=gosper-gun emergent --config ./config/advanced-examples/game-of-life/emergent.toml
```

Available patterns: `glider`, `blinker`, `pulsar`, `r-pentomino`, `acorn`, `gosper-gun`.

To run from a different directory, set the dashboard path:

```bash
export EMERGENT_DASHBOARD_DIR=/path/to/emergent/config/advanced-examples/game-of-life
emergent --config /path/to/emergent/config/advanced-examples/game-of-life/emergent.toml
```

### What this demonstrates

- **Stateful transformation**: The Python handler (`world.py`) maintains a full grid across messages, applying Game of Life rules on each tick. This is the pattern for any handler that needs to accumulate or evolve state over time.
- **One-shot seeding**: The seed source fires once and exits. The timer continues driving the simulation indefinitely. Sources are independent — adding or removing one doesn't affect the others.
- **Emergent complexity**: Four simple rules, one handler, two sinks. Gliders navigate across the grid, oscillators pulse, and spaceships fly — all from a single seed event flowing through a pub-sub pipeline. The complex behavior emerges from the rules, not from the topology.
- **Real-time streaming**: The SSE sink pushes every generation to the browser. The canvas renders incrementally (only changed cells), keeping frame rate smooth at 150ms intervals.
- **Environment-driven configuration**: The `LIFE_PATTERN` variable selects the seed pattern without changing any config or code — the exec-source shell command reads it directly.
