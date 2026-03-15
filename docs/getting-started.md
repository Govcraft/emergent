# Getting Started

*From zero to a running AI pipeline in minutes.*

Emergent turns CLI tools into composable pipeline building blocks. You declare the topology in TOML, and the engine handles process lifecycle, message routing, and graceful shutdown. This guide starts with pre-built marketplace primitives (no code required), then shows how to write custom primitives when you need more control.

---

## 1. Install the Engine

The engine is a single binary. No Rust toolchain, no runtime dependencies.

### Arch Linux (AUR)

```bash
yay -S emergent-bin
```

### Cargo

If you have a Rust toolchain installed:

```bash
cargo install emergent-engine
```

### Download a Pre-built Binary

Download the latest release from [GitHub Releases](https://github.com/Govcraft/emergent/releases/latest):

```bash
# Linux (x86_64)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-x86_64-unknown-linux-gnu.tar.gz
tar xzf emergent-x86_64-unknown-linux-gnu.tar.gz
sudo mv emergent /usr/local/bin/

# Linux (aarch64 / ARM64)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-aarch64-unknown-linux-gnu.tar.gz
tar xzf emergent-aarch64-unknown-linux-gnu.tar.gz
sudo mv emergent /usr/local/bin/

# macOS (Apple Silicon)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-aarch64-apple-darwin.tar.gz
tar xzf emergent-aarch64-apple-darwin.tar.gz
sudo mv emergent /usr/local/bin/

# macOS (Intel)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-x86_64-apple-darwin.tar.gz
tar xzf emergent-x86_64-apple-darwin.tar.gz
sudo mv emergent /usr/local/bin/
```

Verify the installation:

```bash
emergent --help
```

You can also install to `~/.local/bin/` instead of `/usr/local/bin/` (make sure `~/.local/bin` is on your PATH).

### Build from Source (Alternative)

```bash
git clone https://github.com/Govcraft/emergent
cd emergent
cargo build --release
```

This produces the engine binary at `./target/release/emergent` along with example primitives.

## 2. Install Marketplace Primitives

The marketplace ships pre-built primitives you can compose without writing code. The exec primitives are the primary integration mechanism -- they turn any CLI tool into a pipeline building block.

```bash
# See what's available
emergent marketplace list

# Install the core exec primitives
emergent marketplace install exec-source exec-handler exec-sink

# Install others as needed
emergent marketplace install http-source websocket-handler sse-sink topology-viewer
```

Installed primitives are placed in `~/.local/share/emergent/primitives/bin/`.

---

## 3. Run Your First Pipeline

Create a pipeline that runs `date` every 5 seconds and pretty-prints the output. No code, just TOML.

Create `emergent.toml`:

```toml
[engine]
name = "my-pipeline"
socket_path = "auto"

[[sources]]
name = "ticker"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "5000"]
publishes = ["exec.output"]

[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "exec.output", "--", "jq", "."]
subscribes = ["exec.output"]
```

Run it:

```bash
emergent --config ./emergent.toml
```

You should see the current date printed every 5 seconds:

```json
{
  "command": "date",
  "stdout": "Thu Mar 13 17:23:50 UTC 2025",
  "exit_code": 0
}
```

Stop with Ctrl+C. The engine stops the source first (no new events), then drains the sink (consumes remaining output). No messages lost.

**You built a working pipeline with zero code.** One TOML file, two marketplace primitives, one binary.

---

## 4. Add a Transformation Step

Now add a handler that transforms the data between source and sink. This is where real pipelines take shape -- every handler is a processing step that subscribes to events, transforms them, and publishes new events.

Update `emergent.toml`:

```toml
[engine]
name = "my-pipeline"
socket_path = "auto"

[[sources]]
name = "ticker"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "5000"]
publishes = ["exec.output"]

[[handlers]]
name = "transform"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "exec.output", "--publish-as", "data.transformed", "--", "jq", "-c", "{date: .stdout, processed: true}"]
subscribes = ["exec.output"]
publishes = ["data.transformed"]

[[sinks]]
name = "printer"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "data.transformed", "--", "jq", "."]
subscribes = ["data.transformed"]
```

Now the output is transformed:

```json
{
  "date": "Thu Mar 13 17:23:50 UTC 2025",
  "processed": true
}
```

The handler used jq, but it could be any executable. Replace jq with `claude -p "Summarize this"` and you have an AI pipeline. Replace it with `python3 predict.py` and you have an ML inference step. The tool is incidental -- the engine manages everything else.

---

## 5. Build an AI Pipeline

Here is a concrete example: a pipeline that generates data and sends it through an LLM for analysis.

```toml
[engine]
name = "ai-pipeline"
socket_path = "auto"

# Generate system stats every 30 seconds
[[sources]]
name = "stats"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--shell", "sh", "--command", "uptime | jq -Rc '{uptime: .}'", "--interval", "30000"]
publishes = ["system.stats"]

# Send to an LLM for analysis
[[handlers]]
name = "analyzer"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "system.stats",
    "--publish-as", "analysis.result",
    "--timeout", "30000",
    "--",
    "sh", "-c", "jq -r .uptime | claude -p 'Briefly analyze this system load. Flag anything concerning.' 2>/dev/null"
]
subscribes = ["system.stats"]
publishes = ["analysis.result"]

# Print the analysis
[[sinks]]
name = "output"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "analysis.result", "--", "jq", "-r", ".stdout // ."]
subscribes = ["analysis.result"]
```

Swap the LLM by changing the handler's command:

```toml
# Use local Ollama instead of Claude
args = ["--", "sh", "-c", "input=$(cat) && curl -s http://localhost:11434/api/generate -d '{\"model\":\"llama3\",\"prompt\":'\"$input\"',\"stream\":false}' | jq -r .response"]

# Use a Python ML model instead of an LLM
args = ["--", "python3", "analyze.py"]

# Use a simple heuristic
args = ["--", "sh", "-c", "jq -r '.uptime' | awk '{if ($NF > 2.0) print \"HIGH LOAD\"; else print \"OK\"}'"]
```

The model or tool behind the handler is your choice. Emergent does not know or care what runs inside -- it manages the process, routes the messages, and handles the lifecycle.

---

## 6. Understanding the Configuration

**The configuration file is your architecture diagram.** Everything about your pipeline lives in one TOML file: which processes run, what they publish and subscribe to, how they connect. New team members can read this file and understand your entire workflow -- no code diving required.

### Engine Settings

```toml
[engine]
name = "emergent"
socket_path = "auto"              # XDG-compliant default
wire_format = "messagepack"       # or "json" for debugging
api_port = 8891                   # HTTP topology API (0 to disable)
```

### Event Store

```toml
[event_store]
json_log_dir = "./logs"           # Append-only JSON logs (one per day)
sqlite_path = "./events.db"       # Structured storage for queries
retention_days = 30
```

Every message is persisted before routing. You can replay workflows, trace causation chains, and audit all activity.

### The Three Primitive Types

```toml
# Sources: publish only. Bring data into the system.
[[sources]]
name = "timer"
path = "~/.local/share/emergent/primitives/bin/exec-source"
args = ["--command", "date", "--interval", "5000"]
publishes = ["timer.tick"]

# Handlers: subscribe and publish. Transform data.
[[handlers]]
name = "filter"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = ["-s", "timer.tick", "--publish-as", "timer.filtered", "--", "jq", "."]
subscribes = ["timer.tick"]
publishes = ["timer.filtered"]

# Sinks: subscribe only. Consume data (output, store, forward).
[[sinks]]
name = "console"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "timer.filtered", "--", "jq", "."]
subscribes = ["timer.filtered"]
```

The `path` field points to any executable: a marketplace binary, a compiled Rust program, `python3`, `deno`, or any other command. The engine spawns each primitive as a child process.

See the [Configuration Reference](configuration.md) for all options.

---

## 7. The Three Primitives: Mental Model

Every component fits one of three roles:

- **Source**: Publishes events into the system. Cannot receive messages. Examples: timers, webhooks, file watchers, API pollers.
- **Handler**: Subscribes to events, processes them, publishes new events. This is where transformation logic lives -- filtering, enrichment, model inference, routing.
- **Sink**: Subscribes to events, produces side effects. Cannot publish. Examples: console output, file logging, HTTP calls, database writes, Slack posts.

**Startup order**: Sinks start first (ready to receive), then Handlers, then Sources (produce only when pipeline is ready).

**Shutdown order**: Sources stop first (no new events), Handlers drain pending work, Sinks flush remaining output. Zero message loss.

These patterns are identical across Rust, TypeScript, Python, and Go. Once you learn the model, you know it in all four languages.

---

## 8. When to Write Custom Primitives

The exec primitives handle most use cases: any CLI tool becomes a pipeline building block. Write a custom primitive when you need:

- **Persistent state across messages**: exec-handler is stateless (each message spawns a fresh process). A custom handler can maintain state in memory.
- **Custom protocols**: Binary protocols, streaming connections, or anything beyond stdin/stdout piping.
- **High-performance processing**: Eliminate per-message process spawn overhead for high-throughput workloads.
- **Complex async logic**: Long-running connections, concurrent processing, backpressure.

### Scaffold a Custom Primitive

```bash
# Interactive wizard -- pick language, type, message types
emergent scaffold

# Or script it
emergent scaffold -t handler -n my_filter -l python -S timer.tick -p timer.filtered
```

### Install an SDK

```bash
# TypeScript (Deno)
deno add jsr:@govcraft/emergent

# Python
pip install emergent-client    # or: uv add emergent-client

# Rust
cargo add emergent-client tokio serde_json --features tokio/full

# Go
go get github.com/govcraft/emergent/sdks/go
```

### Quick Examples

**Python Handler** (stateful -- maintains a running average):

```python
from emergent import run_handler, create_message

readings = []

async def process(msg, handler):
    data = msg.payload_as(dict)
    readings.append(data["value"])
    avg = sum(readings) / len(readings)
    await handler.publish(
        create_message("sensor.average")
        .caused_by(msg.id)
        .payload({"average": avg, "count": len(readings)})
    )

import asyncio
asyncio.run(run_handler("averager", ["sensor.reading"], process))
```

**TypeScript Sink**:

```typescript
import { runSink } from "jsr:@govcraft/emergent";

await runSink("my_sink", ["analysis.result"], async (msg) => {
  const data = msg.payloadAs<{ summary: string }>();
  console.log(`Analysis: ${data.summary}`);
});
```

The SDK helpers (`run_source`, `run_handler`, `run_sink`) handle connection, signal handling, and graceful shutdown. You write only the business logic.

See the full [SDK documentation](sdks/) for API references in all four languages.

---

## 9. Next Steps

**Explore the examples:**
- [Examples Guide](examples.md) -- Zero-code pipelines (including the full Slack bot) and advanced patterns (fan-in, fan-out, stateful handlers, real-time visualization)

**Learn the architecture:**
- [Concepts](concepts.md) -- Three-primitive model, message format, event sourcing, lifecycle management

**Reference:**
- [Configuration](configuration.md) -- All TOML options, secrets management, multi-language paths
- [Primitives](primitives/) -- Source, Handler, and Sink reference
- [SDKs](sdks/) -- Rust, TypeScript, Python, Go

**Start building:** identify your Sources (where does data come from?), Handlers (what processing is needed?), and Sinks (where does output go?). Start with exec primitives. Reach for custom SDK primitives only when you need persistent state, custom protocols, or high-performance processing.
