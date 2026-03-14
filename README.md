# Emergent

Compose AI-powered automations from CLI tools -- no framework required.

Any command-line tool -- an LLM, a classical ML model, a curl call to an API, a jq transformation -- becomes a composable building block. Wire them together in a TOML file. Emergent handles process lifecycle, message routing, and graceful shutdown. No Python framework. No boilerplate servers. No lock-in.

## What Makes This Different

| | LangChain / CrewAI | Bash scripts | Emergent |
|---|---|---|---|
| Add a new model | Learn framework abstractions | Edit fragile glue code | Add 5 lines of TOML |
| Process crashes | Takes down the chain | Silent failure, zombie processes | Isolated -- rest of pipeline stays alive |
| Swap LLM provider | Rewrite integration layer | Change one command | Change one `args` line |
| Graceful shutdown | Framework-dependent | `kill -9` and hope | Three-phase drain, zero message loss |
| Language lock-in | Python only | Bash only | Any language, any tool |

## Quick Start

```bash
# Install the engine (single binary, no runtime dependencies)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-x86_64-unknown-linux-gnu.tar.gz
tar xzf emergent-x86_64-unknown-linux-gnu.tar.gz
sudo mv emergent /usr/local/bin/

# Install pre-built primitives from the marketplace
emergent marketplace install exec-source exec-handler exec-sink
```

## The Slack Bot: An AI Chatbot in Pure TOML

This is an 8-step Claude-powered Slack chatbot. It connects via WebSocket, receives messages, sends them to Claude, and posts responses back. Zero application code -- just TOML, jq, curl, and a model call.

```
slack-connect ──> url-extractor ──> slack-ws ──> slack-ack (auto-ack envelopes)
                                       │
                                       └──> slack-extract ──> prepare-prompt ──> claude-respond ──> slack-poster
```

The key step -- sending a message to Claude -- is one exec-handler:

```toml
[[handlers]]
name = "claude-respond"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "slack.prompt",
    "--timeout", "60000",
    "--",
    "sh", "-c",
    "input=$(cat) && channel=$(echo \"$input\" | jq -r .channel) && text=$(echo \"$input\" | jq -r .text) && response=$(echo \"$text\" | claude -p 2>/dev/null) && jq -nc --arg channel \"$channel\" --arg text \"$response\" '{channel: $channel, text: $text}'"
]
subscribes = ["slack.prompt"]
publishes = ["slack.post"]
```

Want to use Ollama instead of Claude? Change one line:

```toml
# Claude
"... response=$(echo \"$text\" | claude -p 2>/dev/null) ..."

# Ollama (local)
"... response=$(echo \"$text\" | curl -s http://localhost:11434/api/generate -d \"{\\\"model\\\": \\\"llama3\\\", \\\"prompt\\\": \\\"$text\\\", \\\"stream\\\": false}\" | jq -r .response) ..."

# OpenAI-compatible API
"... response=$(echo \"$text\" | curl -s https://api.openai.com/v1/chat/completions -H \"Authorization: Bearer $OPENAI_API_KEY\" -H 'Content-Type: application/json' -d \"{\\\"model\\\": \\\"gpt-4\\\", \\\"messages\\\": [{\\\"role\\\": \\\"user\\\", \\\"content\\\": \\\"$text\\\"}]}\" | jq -r '.choices[0].message.content') ..."
```

The model behind the handler is incidental. Emergent manages the process, routes the messages, and handles the lifecycle. It does not know or care whether a handler runs a 400B parameter LLM or a hand-tuned regex.

[Full slack-bot config](config/examples/slack-bot.toml) | [Setup instructions](docs/examples.md#slack-bot)

## Tool-Agnostic by Design

The exec-handler pipes event payloads through any executable's stdin and publishes stdout as a new event. The tool is your choice:

```toml
# LLM: Claude CLI
args = ["--", "claude", "-p", "Summarize this JSON data"]

# LLM: Local Ollama via curl
args = ["--", "sh", "-c", "curl -s http://localhost:11434/api/generate -d '{\"model\":\"llama3\",\"prompt\":'$(cat)',\"stream\":false}' | jq .response"]

# Classical ML: Python scikit-learn model
args = ["--", "python3", "predict.py"]

# Data transformation: jq
args = ["--", "jq", ".data | map(select(.score > 0.8))"]

# System utility: any Unix command
args = ["--", "wc", "-l"]
```

Every tool gets the same lifecycle management, message routing, graceful shutdown, and event sourcing. Add a new step to your pipeline by adding a few lines of TOML. Remove a step by deleting them.

## How It Works: Three Primitives

```
Source ──publish──> Handler ──transform──> Sink
  │                    │                     │
  publish only         sub + publish         subscribe only
```

| Primitive | Subscribe | Publish | Purpose |
|-----------|-----------|---------|---------|
| Source    | No        | Yes     | Ingress: emit events (timers, webhooks, APIs) |
| Handler   | Yes       | Yes     | Transform: process and re-emit (filter, enrich, model call) |
| Sink      | Yes       | No      | Egress: consume events (logs, dashboards, API calls) |

That is the entire model. Every workflow is a composition of these three types. Sources cannot receive messages. Sinks cannot produce them. Handlers do both. This constraint makes pipelines predictable: you can read the TOML config and understand exactly what data flows where.

## Marketplace: Pre-Built Primitives

Install pre-built primitives and compose pipelines without writing code:

```bash
emergent marketplace list
emergent marketplace install exec-handler
emergent marketplace info exec-handler
```

| Primitive | Kind | Description |
|-----------|------|-------------|
| `exec-source` | Source | Run any shell command on an interval, emit output as events |
| `http-source` | Source | Receive HTTP webhooks |
| `exec-handler` | Handler | Pipe event payloads through any executable |
| `websocket-handler` | Handler | Bidirectional WebSocket bridge |
| `exec-sink` | Sink | Pipe event payloads through any executable (fire-and-forget) |
| `sse-sink` | Sink | Push events to browsers via Server-Sent Events |
| `topology-viewer` | Sink | Real-time D3.js pipeline visualization |

## Zero-Code Pipeline Examples

### Basic Pipeline

Run `date` every 3 seconds, transform with jq, pretty-print the result:

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

```bash
emergent marketplace install exec-source exec-handler exec-sink
emergent --config ./config/examples/basic-pipeline.toml
```

### AI Chatbot (Slack)

Eight marketplace primitives, zero custom code. See [slack-bot.toml](config/examples/slack-bot.toml).

### Self-Seeding Loop

Subscribes to its own startup event, bootstraps a loop that circulates forever with an incrementing counter. See [ouroboros-loop.toml](config/examples/ouroboros-loop.toml).

### WebSocket Echo

Connects to a WebSocket echo server, sends a message, prints the round-trip response. See [websocket-echo.toml](config/examples/websocket-echo.toml).

## Advanced Examples

Pipelines demonstrating fan-in, fan-out, stateful transformation, and real-time browser visualization.

| Example | What It Demonstrates |
|---------|---------------------|
| [system-monitor](config/advanced-examples/system-monitor/) | Six metric sources fan into a stateful Python handler, fan out to an SSE dashboard and console |
| [game-of-life](config/advanced-examples/game-of-life/) | Conway's Game of Life as a pub-sub pipeline -- gliders emerge from four rules applied to a message stream |
| [reaction-diffusion](config/advanced-examples/reaction-diffusion/) | Gray-Scott Turing patterns computed by a parallel Rust handler, streamed to a browser canvas via SSE |

![System Monitor Dashboard](docs/images/system-monitor.png)
*Six metric sources fan into a stateful handler, fan out to a live SSE dashboard*

![Game of Life](docs/images/game-of-life.png)
*Gliders and oscillators emerge from four rules applied to a pub-sub message stream*

![Reaction-Diffusion](docs/images/reaction-diffusion.png)
*Gray-Scott Turing patterns computed by a parallel Rust handler*

```bash
emergent marketplace install exec-source exec-sink sse-sink
emergent --config ./config/advanced-examples/game-of-life/emergent.toml
# Open http://localhost:8082 to watch
```

See the [Examples Guide](docs/examples.md) for full setup instructions and pattern explanations.

## Topology Viewer

See your running pipeline -- nodes, subscriptions, and process state -- in real time:

![Emergent Topology Viewer](docs/images/topology-viewer.png)

```bash
emergent marketplace install topology-viewer
```

## Write Your Own Primitives

When exec primitives are not enough -- you need persistent state across messages, custom protocols, or high-performance processing -- write a custom primitive in any supported language. The SDKs for Rust, TypeScript, Python, and Go expose identical patterns.

### Scaffold a Primitive

```bash
# Interactive wizard
emergent scaffold

# Or use flags
emergent scaffold -t handler -n my_filter -l python -S timer.tick -p timer.filtered
```

### TypeScript

```typescript
import { runSink } from "jsr:@govcraft/emergent";

await runSink("my_sink", ["sensor.reading"], async (msg) => {
  const data = msg.payloadAs<{ temperature: number }>();
  console.log(`Temperature: ${data.temperature}`);
});
```

### Python

```python
from emergent import run_handler, create_message

async def enrich(msg, handler):
    data = msg.payload_as(dict)
    enriched = {**data, "processed_by": "python"}
    await handler.publish(
        create_message("data.enriched").caused_by(msg.id).payload(enriched)
    )

import asyncio
asyncio.run(run_handler("enricher", ["data.raw"], enrich))
```

### Rust

```rust
use emergent_client::{EmergentSource, EmergentMessage};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = EmergentSource::connect("my_source").await?;

    let message = EmergentMessage::new("sensor.reading")
        .with_payload(json!({"temperature": 72.5}));

    source.publish(message).await?;
    Ok(())
}
```

### Go

```go
package main

import (
    "context"
    emergent "github.com/govcraft/emergent/sdks/go"
)

func main() {
    emergent.RunSource("my_source", func(ctx context.Context, source *emergent.EmergentSource) error {
        msg, _ := emergent.NewMessage("sensor.reading")
        msg.WithPayload(map[string]any{"temperature": 72.5})
        return source.Publish(msg)
    })
}
```

## Features

- **Tool-agnostic composition**: Any CLI tool or API call becomes a pipeline building block via exec primitives
- **TOML-as-architecture**: Your config file is your entire pipeline topology -- readable, auditable, versionable
- **Built-in marketplace**: Install pre-built primitives as binaries with `emergent marketplace install`
- **Process isolation**: Each primitive runs as its own process -- a crashed model call cannot take down the pipeline
- **Built-in event sourcing**: Every message logged with causation chains for debugging and replay
- **Graceful lifecycle management**: Three-phase shutdown (sources stop, handlers drain, sinks drain) with zero message loss
- **Polyglot SDKs**: Write custom primitives in Rust, TypeScript, Python, or Go when exec is not enough
- **Simple IPC protocol**: MessagePack over Unix sockets -- no distributed systems setup

## Documentation

- **[Getting Started](docs/getting-started.md)** -- Install, run your first pipeline, extend it
- **[Examples](docs/examples.md)** -- Zero-code pipelines and advanced patterns
- **[Concepts](docs/concepts.md)** -- Architecture, message flow, event sourcing
- **[Primitives](docs/primitives/)** -- Reference for Sources, Handlers, Sinks
- **[Configuration](docs/configuration.md)** -- All configuration options
- **[SDKs](docs/sdks/)** -- Rust, TypeScript, Python, Go

## Requirements

The engine is a single pre-built binary. No Rust toolchain required.

To write custom primitives, install the SDK for your language:
- **TypeScript**: Deno 1.40+
- **Python**: Python 3.11+ with uv
- **Rust**: Rust 1.75+
- **Go**: Go 1.23+

## License

MIT
