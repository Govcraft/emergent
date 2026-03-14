# Audience Analysis

This document defines the primary and secondary audiences for Emergent, informed by the project's actual capabilities rather than how the docs have historically presented it.

## Key Insight: Tool-Agnostic AI/ML Orchestration

The `exec-*` primitives turn Emergent into **Unix pipes with lifecycle management**. Any CLI tool or API call becomes a composable building block in a TOML file. This includes but is not limited to:

- `claude -p` or any LLM CLI
- `curl` to Ollama, OpenAI, Anthropic, or any HTTP API
- A Python script running a classical ML model trained with AutoGluon, scikit-learn, or XGBoost
- A hand-rolled statistical model invoked as a binary
- `jq`, `awk`, `sed`, or any standard Unix tool
- A custom Rust/Go/Python executable

The model or tool behind an exec-handler is incidental. Emergent doesn't know or care whether a handler is running a 400B parameter LLM or a hand-tuned regex. It manages the process, routes the messages, and handles the lifecycle. This makes it a universal orchestration layer for any computation that can be invoked from a command line.

---

## Primary Audiences

### 1. AI/ML Automation Builders

People who want to wire models and AI tools into real workflows without adopting a framework.

**Who they are:** Developers building chatbots, agents, AI-powered integrations, ML inference pipelines. Their current alternatives are heavyweight Python frameworks (LangChain, CrewAI, AutoGen) with steep learning curves and ecosystem lock-in — or fragile bash scripts gluing things together.

**What Emergent gives them:**

- Any CLI-invocable model (LLM, classical, or otherwise) becomes a handler via exec-handler
- TOML config = the entire pipeline architecture, readable at a glance
- Process isolation = if a model call hangs or crashes, the rest of the pipeline stays alive
- Composability scales linearly: add sentiment analysis before the LLM? Add one handler. Log every interaction? Add a sink. Add a second input channel? Add a source. The config grows by a few lines, not a new abstraction layer.

**Proof point:** The [slack-bot example](../config/examples/slack-bot.toml) is an 8-step AI chatbot with WebSocket connectivity, LLM processing, and Slack API integration — zero application code, just TOML + jq + curl + a model call.

**Pain they feel today:**
- Framework lock-in (LangChain requires their abstractions, their Python, their way)
- Boilerplate explosion (Express server + WebSocket library + queue + process manager + error handling)
- Fragile glue scripts (bash pipelines with no lifecycle management, no graceful shutdown, no observability)

### 2. CLI-Native Automation Developers

People who think in Unix pipes and want lifecycle management on top.

**Who they are:** Developers and operators wiring together shell commands, APIs, and tools. They'd otherwise use bash scripts, cron jobs, or Makefiles — and accept the fragility. They're comfortable with `jq`, `curl`, `sh -c`, and environment variables.

**What Emergent gives them:**

- The mental model they already have (stdin/stdout piping) with graceful shutdown, restart, event sourcing, and fan-in/fan-out
- exec primitives as the universal adapter: if it runs in a shell, it's a primitive
- TOML configuration replaces fragile bash orchestration
- Built-in event store provides replay and audit without additional infrastructure

**Proof points:** The [basic-pipeline](../config/examples/basic-pipeline.toml) (date + jq + pretty-print) and [ouroboros-loop](../config/examples/ouroboros-loop.toml) (self-seeding infinite loop) examples are pure CLI composition with zero custom code.

**Pain they feel today:**
- Bash scripts that silently fail, leave zombie processes, or lose data on crash
- No graceful shutdown — kill -9 and hope for the best
- No fan-in or fan-out without writing explicit routing logic
- No event history or replay capability

### 3. Custom Primitive Developers (SDK Users)

People who outgrow exec primitives and need stateful handlers, custom protocols, or high-performance processing.

**Who they are:** Backend and systems developers who need more than exec primitives can provide — persistent state across messages, custom binary protocols, parallel computation, or tight performance budgets. They pick the SDK for their language (Rust, Python, TypeScript, Go) and write a proper primitive.

**What Emergent gives them:**

- SDKs in four languages with identical mental models
- Scaffold command generates boilerplate
- The same lifecycle management and message routing as exec primitives
- Freedom to use language-native libraries (Pydantic in Python, rayon in Rust, channels in Go)

**Proof points:** The [system-monitor](../config/advanced-examples/system-monitor/) (Python handler computing per-second deltas with state), [game-of-life](../config/advanced-examples/game-of-life/) (Python stateful grid handler), and [reaction-diffusion](../config/advanced-examples/reaction-diffusion/) (Rust handler with rayon parallelism) examples.

**Pain they feel today:**
- Building process management, IPC, and lifecycle from scratch for every project
- Coordinating polyglot services without Kubernetes or a message broker
- Testing event-driven systems without a replay mechanism

---

## Secondary Audiences

### 4. Platform / DevOps Engineers

The operators, not necessarily the builders.

**Who they are:** Engineers responsible for deploying and running workflows in production. They care about systemd integration, process lifecycle, graceful shutdown, observability, and the event store for audit trails.

**What Emergent gives them:**

- Single pre-built binary, no runtime dependencies
- Systemd deployment with standard service management
- Three-phase graceful shutdown (sources stop, handlers drain, sinks drain)
- Event store for compliance and debugging
- Topology API for monitoring and health checks

**Note:** The current docs over-index on this audience. They are important but they are not the people who discover or choose Emergent — they operate what audiences 1-3 build.

### 5. Creative Technologists / Explorers

People exploring emergent behavior, simulations, generative art, and educational visualizations.

**Who they are:** Hobbyists, educators, creative coders, and researchers who find the pub-sub + process isolation model a natural fit for simulations and interactive systems.

**What Emergent gives them:**

- A simulation clock as a source, world state as a handler, visualization as a sink — clean separation
- SSE sinks for real-time browser rendering
- Mix languages per component (Rust for compute-heavy simulation, Python for seeding logic)

**Proof points:** Game of Life and reaction-diffusion examples. These are genuinely compelling demonstrations and make excellent showcase material.

---

## Implications for Documentation

| Aspect | Current docs | Should be |
|---|---|---|
| Headline positioning | "Lightweight workflow engine" | "Compose AI-powered automations from CLI tools — no framework required" |
| Lead example | basic-pipeline (`date` + jq) | AI chatbot or ML inference pipeline via exec primitives |
| Primary entry point | "Write your own primitives" (SDK) | "Install marketplace primitives, compose with TOML" (zero-code) |
| AI/ML story | Absent | Front and center — tool-agnostic model orchestration |
| exec primitives | Listed as marketplace items | Positioned as the primary integration mechanism |
| Custom SDK primitives | Presented as the default path | Presented as the escape hatch when exec isn't enough |
| Framework comparison | None | Explicit contrast with LangChain/CrewAI (lighter, polyglot, no lock-in) |

### Missing Examples to Create

- ML inference pipeline (classical model via exec-handler)
- Multi-model pipeline (e.g., transcription -> summarization -> classification)
- Webhook-triggered AI processing (HTTP source -> model handler -> API sink)
- Monitoring/alerting pipeline with AI-powered anomaly detection
