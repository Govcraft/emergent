# Advanced Examples

Pipelines that demonstrate event-driven patterns beyond linear chains: fan-in, fan-out, and real-time browser integration.

## system-monitor

Three sources poll disk, memory, and load metrics on independent intervals. All converge on a single formatter handler (fan-in), which fans out to a live SSE dashboard and console output.

```
exec-source (df)     ──┐
exec-source (free)   ──┼→ exec-handler (format) ──┬→ sse-sink (browser dashboard)
exec-source (uptime) ──┘                           └→ exec-sink (console)
```

### Prerequisites

```bash
emergent marketplace install exec-source exec-handler exec-sink sse-sink
```

### Usage

```bash
# Terminal 1: serve the dashboard
cd config/advanced-examples/system-monitor
python3 -m http.server 8080

# Terminal 2: run the pipeline
emergent --config ./config/advanced-examples/system-monitor/emergent.toml
```

Open http://localhost:8080/dashboard.html to see live metrics. The SSE sink pushes updates on port 8081.

### What this demonstrates

- **Fan-in**: Three independent sources converge on one handler. No routing logic — they all publish metric types, the handler subscribes to all of them.
- **Fan-out**: One handler's output goes to multiple sinks simultaneously. Add a third sink (Slack alerts, file logging) without modifying any existing primitive.
- **Independent intervals**: Each source polls at its own rate. Memory and load every 5 seconds, disk every 10. No orchestrator coordinating timing.
- **Decoupled extensibility**: Add a new metric source (Docker stats, network IO) by adding one `[[sources]]` block. Nothing else changes.
