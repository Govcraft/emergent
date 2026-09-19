# Emergent Implementation Templates

## Marketplace Primitive Patterns (Zero Code)

### Basic Pipeline: Poll → Transform → Print

```toml
[engine]
name = "basic-pipeline"
socket_path = "auto"

[[sources]]
name = "ticker"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "3000"]
publishes = ["exec.output"]

[[handlers]]
name = "transform"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "exec.output", "--publish-as", "data.transformed", "--", "jq", "-c", ". + {transformed: true}"]
subscribes = ["exec.output"]
publishes = ["data.transformed"]

[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "data.transformed", "--", "jq", "."]
subscribes = ["data.transformed"]
```

### API Polling → Filter → Alert

```toml
[engine]
name = "api-monitor"
socket_path = "auto"

# Poll an API every 30 seconds
[[sources]]
name = "api-poll"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--shell", "sh", "--command", "curl -s https://api.example.com/status | jq -c .", "--interval", "30000"]
publishes = ["api.status"]

# Filter: only pass through errors
[[handlers]]
name = "error-filter"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "api.status", "--publish-as", "api.error", "--", "jq", "-c", "select(.stdout | fromjson | .status != \"ok\") | .stdout | fromjson"]
subscribes = ["api.status"]
publishes = ["api.error"]

# Alert via webhook
[[sinks]]
name = "alerter"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "api.error", "--", "sh", "-c", "jq -c . | curl -s -X POST -H 'Content-Type: application/json' -d @- https://hooks.slack.com/services/YOUR/WEBHOOK/URL"]
subscribes = ["api.error"]
```

### HTTP Webhook → LLM → Response

```toml
[engine]
name = "ai-webhook"
socket_path = "auto"

[[sources]]
name = "webhook"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "8080"]
publishes = ["http.request"]

# Extract the question from the webhook body
[[handlers]]
name = "extract"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "http.request", "--", "jq", "-c", ".body.question // .body.text // .body"]
subscribes = ["http.request"]
publishes = ["ai.prompt"]

# Send to Claude
[[handlers]]
name = "claude"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "ai.prompt", "--timeout", "60000", "--", "sh", "-c", "cat | claude -p 2>/dev/null | jq -Rsc '{response: .}'"]
subscribes = ["ai.prompt"]
publishes = ["ai.response"]

[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "ai.response", "--", "jq", "."]
subscribes = ["ai.response"]
```

### WebSocket Bridge

```toml
[engine]
name = "ws-bridge"
socket_path = "auto"
api_port = 0

# Emit connect command
[[sources]]
name = "connector"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "echo", "--args", "{\"url\":\"wss://example.com/ws\"}"]
publishes = ["exec.output"]

# Extract URL from exec-source stdout wrapper
[[handlers]]
name = "url-extractor"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "exec.output", "--", "jq", "-c", ".stdout | fromjson"]
subscribes = ["exec.output"]
publishes = ["ws.connect"]

# WebSocket handler
[[handlers]]
name = "ws"
path = "~/.local/share/emergent/primitives/bin/websocket-handler"
args = ["--prefix", "ws"]
subscribes = ["ws.connect", "ws.send"]
publishes = ["ws.connected", "ws.frame", "ws.closed", "ws.error"]

# React to connection: send a message
[[handlers]]
name = "on-connect"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "ws.connected", "--", "jq", "-c", "-n", "{data: {hello: \"world\"}}"]
subscribes = ["ws.connected"]
publishes = ["ws.send"]

# Print frames
[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "ws.frame", "--", "jq", "."]
subscribes = ["ws.frame"]
```

### Self-Seeding Feedback Loop

```toml
[engine]
name = "ouroboros"
socket_path = "auto"
api_port = 0

[[sources]]
name = "webhook"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--host", "127.0.0.1", "--port", "8088"]
publishes = ["http.request"]

[[handlers]]
name = "enrich"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "http.request", "--publish-as", "loop.iteration", "--", "jq", "-c", ".body.count += 1 | .body.timestamp = now"]
subscribes = ["http.request"]
publishes = ["loop.iteration"]

[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "loop.iteration", "--", "jq", "."]
subscribes = ["loop.iteration"]

# Seeds on startup AND loops back on each iteration. One act (the POST); the
# sleep is the loop's clock, and --retry-connrefused covers a webhook that has
# been spawned but has not bound its port yet.
[[sinks]]
name = "loopback"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "loop.iteration", "-s", "system.started.webhook", "--",
    "sh", "-c",
  '''sleep 1; jq -c '.body // {count: 0}' \
     | curl -s --retry 5 --retry-connrefused -X POST -H 'Content-Type: application/json' -d @- http://127.0.0.1:8088''']
subscribes = ["loop.iteration", "system.started.webhook"]
```

### Fan-In Metrics with Live Dashboard

```toml
[engine]
name = "metrics"
socket_path = "auto"
api_port = 0

[[sources]]
name = "cpu"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--shell", "sh", "--command", "echo '{\"metric\":\"cpu\",\"value\":42}'", "--interval", "1000"]
publishes = ["metric.cpu"]

[[sources]]
name = "memory"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--shell", "sh", "--command", "echo '{\"metric\":\"memory\",\"value\":64}'", "--interval", "2000"]
publishes = ["metric.memory"]

# Fan-in: one handler subscribes to all metric types
[[handlers]]
name = "normalize"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "metric.cpu", "-s", "metric.memory", "--publish-as", "monitor.metric", "--",
    "jq", "-c", ".stdout | fromjson | . + {timestamp: now}"]
subscribes = ["metric.cpu", "metric.memory"]
publishes = ["monitor.metric"]

# Fan-out: SSE dashboard + console
[[sinks]]
name = "dashboard"
path = "~/.local/share/emergent/primitives/bin/sse-sink"
args = ["--port", "8081"]
subscribes = ["monitor.metric"]

[[sinks]]
name = "console"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "monitor.metric", "--", "jq", "-c", "[.metric, .value] | join(\": \")"]
subscribes = ["monitor.metric"]
```

## Custom SDK Templates (When Exec Isn't Enough)

Use these when you need **persistent state**, **complex computation**, or **custom protocols**.

### Rust: Stateful Handler

This uses the low-level loop rather than `run_handler`. The helper's current
bound rejects a closure that borrows `handler` across an `.await`, so a handler
that publishes does not compile with it (see `sdk-api.md`). The loop also owns
its state outright, so it needs no `Arc` or `Mutex`.

```rust
use emergent_client::{EmergentHandler, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut handler = EmergentHandler::connect("counter_handler").await?;
    let mut stream = handler.subscribe(["input.event"]).await?;

    let mut count = 0u64;

    // Ends when the engine sends system.shutdown or the connection closes.
    while let Some(msg) = stream.next().await {
        count += 1;
        let output = EmergentMessage::new("output.counted")
            .with_causation_from_message(msg.id())
            .with_payload(json!({"count": count, "input": msg.payload()}));
        handler.publish(output).await?;
    }

    handler.disconnect().await?;
    Ok(())
}
```

If state must be shared with another task, never hold a `std::sync::MutexGuard`
across an `.await` (the future stops being `Send`). Take the value out in a
block first: `let n = { let mut c = counter.lock().map_err(|e| e.to_string())?; *c += 1; *c };`.

### Python: Stateful Handler

```python
from emergent import run_handler, create_message

state = {"count": 0, "history": []}

async def process(msg, handler):
    data = msg.payload_as(dict)
    state["count"] += 1
    state["history"].append(data)

    # Keep only last 100 entries
    if len(state["history"]) > 100:
        state["history"] = state["history"][-100:]

    await handler.publish(
        create_message("data.enriched")
        .caused_by(msg.id)
        .payload({
            **data,
            "total_seen": state["count"],
            "history_length": len(state["history"])
        })
    )

import asyncio
asyncio.run(run_handler("enricher", ["data.raw"], process))
```

### Rust: Custom Source (using helpers)

```rust
use emergent_client::helpers::run_source;
use emergent_client::EmergentMessage;
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_source(Some("my_source"), |source, mut shutdown| async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        let mut seq = 0u64;

        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = interval.tick() => {
                    seq += 1;
                    let msg = EmergentMessage::new("sensor.reading")
                        .with_payload(json!({"sequence": seq, "value": 42.0}));
                    source.publish(msg).await.map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }).await?;
    Ok(())
}
```

### Rust: Custom Sink (using helpers)

```rust
use emergent_client::helpers::run_sink;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_sink(
        Some("my_sink"),
        &["sensor.reading"],
        |msg| async move {
            let payload = msg.payload();
            println!("[{}] {}: {}", msg.message_type().as_str(), msg.source().as_str(), payload);
            Ok(())
        }
    ).await?;
    Ok(())
}
```

## Cargo.toml for Custom Primitives

```toml
[package]
name = "my_primitive"
version = "0.1.0"
edition = "2024"

[dependencies]
emergent-client = "0.13"  # Use latest from crates.io
tokio = { version = "1", features = ["full", "signal"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# For custom errors
# thiserror = "2"

# For HTTP sources/sinks
# axum = "0.8"       # HTTP server (sources)
# reqwest = "0.12"   # HTTP client (sinks)
```
