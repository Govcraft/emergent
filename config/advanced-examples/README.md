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
