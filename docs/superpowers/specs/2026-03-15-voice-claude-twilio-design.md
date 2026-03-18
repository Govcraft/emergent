# Voice-Claude Twilio Pipeline — Design Spec

**Date:** 2026-03-15
**Status:** Draft

## Goal

Build a turn-based phone pipeline so users can call Claude/Ollama over the phone (and trigger outbound calls), using Emergent as the orchestration engine across two machines: a public-facing edge node and a GPU inference server.

## Constraints

- **penny-pi** (ARM, `ssh root@penny-pi`): Runs Traefik for public HTTPS ingress. No GPU. Handles Twilio I/O and audio gating.
- **govcraft** (x86_64, CUDA GPU): Runs local inference — whisper.cpp (STT), Ollama (LLM), piper (TTS). Not publicly exposed.
- Both machines are on Tailscale. Inter-engine traffic flows over Tailscale private IPs.
- All inference runs locally — no cloud AI APIs.

## Architecture

Two independent Emergent engines communicate via HTTP bridge (http-source + exec-sink with curl).

```
penny-pi ("voice-edge")                          govcraft ("voice-infer")
Traefik → public HTTPS                           Tailscale-only

┌────────────────────────────────┐    HTTP    ┌──────────────────────────────────┐
│                                │           │                                  │
│ twilio-bridge (handler)        │           │ http-source (:9100)              │
│   POST /voice → TwiML          │           │   publishes: infer.request.raw   │
│   WS /ws → audio.frame         │           │                                  │
│   subscribes: audio.out        │           │ unwrap (exec-handler)            │
│                                │           │   jq .body                       │
│ audio-gate (handler)           │           │   publishes: infer.request       │
│   audio.frame → audio.utterance│           │                                  │
│   keyed by stream_sid          │           │ transcribe (exec-handler)        │
│                                │           │   ffmpeg | whisper-cli -f -      │
│ bridge-out (exec-sink)         │           │   publishes: audio.transcript    │
│   audio.utterance ─────────────╋──POST────→│                                  │
│     curl govcraft:9100         │           │ llm-respond (exec-handler)       │
│                                │           │   curl ollama /api/chat          │
│ http-source (:9200)            │           │   publishes: llm.response        │
│   publishes: bridge.response   │           │                                  │
│                                │←──POST────╋─ synthesize (exec-handler)       │
│ unwrap (exec-handler)          │           │   piper --output-raw | ffmpeg    │
│   jq .body                     │           │   publishes: audio.out           │
│   publishes: audio.out         │           │                                  │
│                                │           │ bridge-out (exec-sink)           │
│ twilio-bridge (subscribes      │           │   audio.out → curl penny-pi:9200 │
│   audio.out → WS send)         │           │                                  │
│                                │           │ console (exec-sink)              │
│ trigger-source (http-source)   │           │   debug logging                  │
│   :9300 local-only             │           │                                  │
│   publishes: call.outbound.req │           │                                  │
│                                │           │                                  │
│ outbound-caller (exec-sink)    │           │                                  │
│   call.outbound.requested      │           │                                  │
│   → Twilio REST API            │           │                                  │
│                                │           │                                  │
│ console (exec-sink)            │           │                                  │
│   debug logging                │           │                                  │
└────────────────────────────────┘           └──────────────────────────────────┘
```

## Design Decisions

### 1. Two engines, not one

Separating edge (Twilio I/O) from inference (GPU compute) keeps each engine focused and independently deployable. penny-pi stays lightweight. govcraft stays off the public internet. The HTTP bridge is the only coupling point.

### 2. http-source + exec-sink for the bridge

Emergent's marketplace http-source accepts POST requests and publishes them as events. No dedicated http-sink exists in the marketplace; exec-sink + curl is the idiomatic pattern. Each direction is fire-and-forget — the response arrives asynchronously on the return path.

### 3. jq .body unwrapper

http-source wraps POST bodies in an envelope: `{method, path, headers, body, remote_addr}`. A single exec-handler running `jq .body` extracts the payload so downstream handlers work with clean data. This is a one-liner, adds negligible latency, and keeps all other handlers unaware of the transport layer.

### 4. Unix pipe composition — no temp files

whisper.cpp supports `-f -` (read raw s16le PCM from stdin). piper supports `--output-raw` (write raw PCM to stdout). Combined with ffmpeg for format conversion, the entire inference pipeline is shell pipes with no intermediate files.

Each exec-handler receives the **full JSON payload** on stdin. The shell commands must: (a) extract fields with jq, (b) run the audio/inference tool, and (c) emit a JSON object with `stream_sid` preserved.

**Transcribe (one exec-handler — decode + STT):**
```sh
input=$(cat) \
  && sid=$(echo "$input" | jq -r .stream_sid) \
  && text=$(echo "$input" | jq -r .mulaw_hex | xxd -r -p \
    | ffmpeg -loglevel quiet -f mulaw -ar 8000 -ac 1 -i - -ar 16000 -ac 1 -f s16le - \
    | whisper-cli -m "$WHISPER_MODEL" -f - 2>/dev/null \
    | grep -v '^\[' | xargs) \
  && jq -n --arg t "$text" --arg s "$sid" '{text: $t, stream_sid: $s}'
```

**TTS + encode (one exec-handler — synthesize + encode):**
```sh
input=$(cat) \
  && text=$(echo "$input" | jq -r .text) \
  && sid=$(echo "$input" | jq -r .stream_sid) \
  && hex=$(printf '%s' "$text" \
    | piper --model "$PIPER_MODEL" --output-raw 2>/dev/null \
    | ffmpeg -loglevel quiet -f s16le -ar "$PIPER_SAMPLE_RATE" -ac 1 -i - -f mulaw -ar 8000 -ac 1 - \
    | xxd -p | tr -d '\n') \
  && jq -n --arg h "$hex" --arg s "$sid" '{mulaw_hex: $h, stream_sid: $s}'
```

**Why the commands look this way:** exec-handler pipes the JSON payload to stdin and captures stdout. If stdout is valid JSON, it's published as-is; otherwise it's wrapped as `{"output": "..."}`. Binary audio bytes would be corrupted by text-mode capture, so the TTS pipe must hex-encode and JSON-wrap its output. The `stream_sid` must be extracted from input and re-injected into output since the audio tools have no concept of it.

**Note on piper sample rate:** Piper's output sample rate is model-dependent (22050 Hz for `en_US-lessac-medium`, 16000 Hz for some others). The `PIPER_SAMPLE_RATE` env var must match the model. See Environment Variables section.

No intermediate WAV files. No Python scripts on govcraft. No cleanup logic.

### 5. Consolidated twilio-bridge on one port

Both the TwiML HTTP endpoint (`POST /voice`) and Twilio Media Stream WebSocket (`/ws`) run on a single aiohttp server (port 8080). This simplifies Traefik config to one route and eliminates the multi-port Funnel issue from the original plan.

### 6. Per-call audio-gate keyed by stream_sid

The audio-gate maintains a `dict[str, AudioGate]` instead of a global singleton. Each active call gets its own buffer and silence counter. Gates are created on first frame for a given `stream_sid`. Cleanup: twilio-bridge publishes `call.ended {stream_sid}` when Twilio sends a `stop` WebSocket event; audio-gate subscribes to `call.ended` and removes the gate entry immediately.

### 7. Ollama via /api/chat

Uses the chat endpoint with a system message for conversational quality. Each turn is stateless (no conversation history) for v1. The exec-handler constructs the request JSON and parses the response.

### 8. Traefik on penny-pi for public ingress

Traefik routes public HTTPS traffic to twilio-bridge. A minimal Traefik config maps:
- `POST /voice` → `localhost:8080` (TwiML webhook)
- `wss:///ws` → `ws://localhost:8080` (Twilio Media Stream WebSocket upgrade)

Inter-engine HTTP (penny-pi:9200, govcraft:9100) goes over Tailscale private IPs — no public exposure.

### 9. Outbound call trigger

The Emergent engine does not expose a `/publish` HTTP endpoint. To trigger outbound calls externally, a dedicated http-source on penny-pi listens on a local-only port (9300). A `curl -X POST localhost:9300 -d '{"to":"+1xxx"}'` publishes `call.outbound.requested`, which the outbound-caller exec-sink picks up. The trigger script wraps this curl call.

## Message Flow (Single Turn)

```
Twilio phone call
  → Twilio POST /voice (via Traefik)
  → twilio-bridge returns TwiML with <Stream url="wss://penny-pi/ws" />
  → Twilio opens WebSocket, sends 20ms μ-law frames

  → twilio-bridge parses media frames
  → publishes audio.frame {mulaw_hex, stream_sid}

  → audio-gate[stream_sid] accumulates frames
  → detects silence (25 frames ≈ 500ms)
  → publishes audio.utterance {mulaw_hex, stream_sid}

  → exec-sink curls POST to govcraft:9100

  govcraft:
    → http-source publishes infer.request.raw
    → unwrap extracts .body → infer.request {mulaw_hex, stream_sid}
    → transcribe: xxd | ffmpeg | whisper-cli → audio.transcript {text, stream_sid}
    → llm-respond: curl ollama /api/chat → llm.response {text, stream_sid}
    → tts-encode: piper | ffmpeg → audio.out {mulaw_hex, stream_sid}
    → exec-sink curls POST to penny-pi:9200

  penny-pi:
    → http-source publishes bridge.response
    → unwrap extracts .body → audio.out {mulaw_hex, stream_sid}
    → twilio-bridge matches stream_sid to active WebSocket
    → sends base64-encoded μ-law media frame to Twilio
    → caller hears the response
```

## Payload Contracts

All payloads are JSON. The `stream_sid` field threads through the entire pipeline to correlate audio with the correct Twilio call.

| Message Type | Payload | Producer | Consumer |
|---|---|---|---|
| `audio.frame` | `{mulaw_hex, stream_sid}` | twilio-bridge | audio-gate |
| `audio.utterance` | `{mulaw_hex, stream_sid}` | audio-gate | bridge-out (exec-sink) |
| `infer.request.raw` | `{method, path, headers, body: {mulaw_hex, stream_sid}}` | http-source | unwrap |
| `infer.request` | `{mulaw_hex, stream_sid}` | unwrap | transcribe |
| `audio.transcript` | `{text, stream_sid}` | transcribe | llm-respond |
| `llm.response` | `{text, stream_sid}` | llm-respond | tts-encode |
| `audio.out` | `{mulaw_hex, stream_sid}` | tts-encode (govcraft) / unwrap (penny-pi) | bridge-out (govcraft) / twilio-bridge (penny-pi) |
| `bridge.response` | `{method, path, headers, body: {mulaw_hex, stream_sid}}` | http-source (penny-pi) | unwrap (penny-pi) |
| `call.ended` | `{stream_sid}` | twilio-bridge | audio-gate |
| `call.outbound.requested` | `{to: "+1xxx"}` | trigger-source (http-source) | outbound-caller |

## Engine Configurations

**Note on environment variables:** All env vars from the Environment Variables section must be present in the engine's process environment (e.g., `source .env` before `emergent --config`). Child processes inherit the engine's environment. The TOML `env` field is only needed for overrides or handler-specific values.

### emergent-edge.toml (penny-pi)

```toml
[engine]
name = "voice-edge"
socket_path = "auto"
api_port = 8891

# --- twilio-bridge: TwiML webhook + Twilio Media Stream WebSocket ---
# Single aiohttp server on port 8080 (HTTP + WS)
[[handlers]]
name = "twilio-bridge"
path = "python3"
args = ["handlers/twilio-bridge/main.py"]
subscribes = ["audio.out"]
publishes = ["audio.frame", "call.ended"]
env = { TWILIO_BRIDGE_PORT = "8080" }

# --- audio-gate: per-call silence detection ---
[[handlers]]
name = "audio-gate"
path = "python3"
args = ["handlers/audio-gate/main.py"]
subscribes = ["audio.frame", "call.ended"]
publishes = ["audio.utterance"]

# --- bridge-out: ship utterances to govcraft for inference ---
[[sinks]]
name = "bridge-out"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = [
    "-s", "audio.utterance",
    "--",
    "sh", "-c",
    "curl -s -X POST -H 'Content-Type: application/json' -d @- http://$GOVCRAFT_HOST:9100/"
]
subscribes = ["audio.utterance"]

# --- bridge-in: receive inference results from govcraft ---
[[sources]]
name = "bridge-in"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "9200", "--host", "0.0.0.0"]
publishes = ["bridge.response"]

# --- unwrap: extract .body from http-source envelope ---
[[handlers]]
name = "unwrap-response"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "bridge.response",
    "--publish-as", "audio.out",
    "--", "jq", ".body"
]
subscribes = ["bridge.response"]
publishes = ["audio.out"]

# --- trigger-source: local HTTP endpoint for outbound call triggers ---
[[sources]]
name = "trigger-source"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "9300", "--host", "127.0.0.1"]
publishes = ["call.trigger.raw"]

# --- unwrap-trigger: extract .body from trigger envelope ---
[[handlers]]
name = "unwrap-trigger"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "call.trigger.raw",
    "--publish-as", "call.outbound.requested",
    "--", "jq", ".body"
]
subscribes = ["call.trigger.raw"]
publishes = ["call.outbound.requested"]

# --- outbound-caller: place outbound calls via Twilio REST API ---
[[sinks]]
name = "outbound-caller"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = [
    "-s", "call.outbound.requested",
    "--",
    "sh", "-c",
    """input=$(cat) \
    && to=$(echo "$input" | jq -r .to) \
    && curl -s -X POST \
        "https://api.twilio.com/2010-04-01/Accounts/$TWILIO_ACCOUNT_SID/Calls.json" \
        --data-urlencode "To=$to" \
        --data-urlencode "From=$TWILIO_PHONE_NUMBER" \
        --data-urlencode "Url=$PUBLIC_URL/voice" \
        -u "$TWILIO_ACCOUNT_SID:$TWILIO_AUTH_TOKEN" > /dev/null"""
]
subscribes = ["call.outbound.requested"]

# --- console: debug logging ---
[[sinks]]
name = "console"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = [
    "-s", "audio.utterance,audio.out,call.ended,system.error.*",
    "--", "jq", "-c", "."
]
subscribes = ["audio.utterance", "audio.out", "call.ended", "system.error.*"]
```

### emergent-infer.toml (govcraft)

```toml
[engine]
name = "voice-infer"
socket_path = "auto"
api_port = 8892

# --- infer-in: receive audio from penny-pi ---
[[sources]]
name = "infer-in"
path = "~/.local/share/emergent/primitives/bin/http-source"
args = ["--port", "9100", "--host", "0.0.0.0"]
publishes = ["infer.request.raw"]

# --- unwrap: extract .body from http-source envelope ---
[[handlers]]
name = "unwrap-request"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "infer.request.raw",
    "--publish-as", "infer.request",
    "--", "jq", ".body"
]
subscribes = ["infer.request.raw"]
publishes = ["infer.request"]

# --- transcribe: μ-law → ffmpeg decode → whisper STT ---
[[handlers]]
name = "transcribe"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "infer.request",
    "--publish-as", "audio.transcript",
    "--timeout", "30000",
    "--",
    "sh", "-c",
    """input=$(cat) \
    && sid=$(echo "$input" | jq -r .stream_sid) \
    && text=$(echo "$input" | jq -r .mulaw_hex | xxd -r -p \
      | ffmpeg -loglevel quiet -f mulaw -ar 8000 -ac 1 -i - -ar 16000 -ac 1 -f s16le - \
      | whisper-cli -m "$WHISPER_MODEL" -f - 2>/dev/null \
      | grep -v '^\[' | xargs) \
    && jq -n --arg t "$text" --arg s "$sid" '{text: $t, stream_sid: $s}'"""
]
subscribes = ["infer.request"]
publishes = ["audio.transcript"]

# --- llm-respond: send transcript to Ollama ---
[[handlers]]
name = "llm-respond"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "audio.transcript",
    "--publish-as", "llm.response",
    "--timeout", "60000",
    "--",
    "sh", "-c",
    """input=$(cat) \
    && text=$(echo "$input" | jq -r .text) \
    && sid=$(echo "$input" | jq -r .stream_sid) \
    && response=$(curl -s http://localhost:11434/api/chat \
        -d "$(jq -n --arg m "$OLLAMA_MODEL" --arg t "$text" \
        '{model: $m, messages: [{role: "system", content: "You are a helpful voice assistant. Be concise — 1 to 3 sentences max."}, {role: "user", content: $t}], stream: false}')" \
        | jq -r '.message.content') \
    && jq -n --arg r "$response" --arg s "$sid" '{text: $r, stream_sid: $s}'"""
]
subscribes = ["audio.transcript"]
publishes = ["llm.response"]

# --- tts-encode: piper TTS → ffmpeg encode to μ-law ---
[[handlers]]
name = "tts-encode"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "llm.response",
    "--publish-as", "audio.out",
    "--timeout", "30000",
    "--",
    "sh", "-c",
    """input=$(cat) \
    && text=$(echo "$input" | jq -r .text) \
    && sid=$(echo "$input" | jq -r .stream_sid) \
    && hex=$(printf '%s' "$text" \
      | piper --model "$PIPER_MODEL" --output-raw 2>/dev/null \
      | ffmpeg -loglevel quiet -f s16le -ar "$PIPER_SAMPLE_RATE" -ac 1 -i - -f mulaw -ar 8000 -ac 1 - \
      | xxd -p | tr -d '\n') \
    && jq -n --arg h "$hex" --arg s "$sid" '{mulaw_hex: $h, stream_sid: $s}'"""
]
subscribes = ["llm.response"]
publishes = ["audio.out"]

# --- bridge-out: ship results back to penny-pi ---
[[sinks]]
name = "bridge-out"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = [
    "-s", "audio.out",
    "--",
    "sh", "-c",
    "curl -s -X POST -H 'Content-Type: application/json' -d @- http://$PENNY_PI_HOST:9200/"
]
subscribes = ["audio.out"]

# --- console: debug logging ---
[[sinks]]
name = "console"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = [
    "-s", "audio.transcript,llm.response,audio.out,exec.error",
    "--", "jq", "-c", "."
]
subscribes = ["audio.transcript", "llm.response", "audio.out", "exec.error"]
```

## File Structure

```
voice-claude/
├── emergent-edge.toml              # penny-pi engine config
├── emergent-infer.toml             # govcraft engine config
├── .env.example                    # template for secrets/paths
├── .gitignore
├── handlers/
│   ├── twilio-bridge/
│   │   ├── main.py                 # TwiML + WebSocket server (aiohttp, one port)
│   │   ├── requirements.txt        # aiohttp, websockets, emergent-client
│   │   └── tests/
│   │       └── test_twilio_bridge.py
│   └── audio-gate/
│       ├── main.py                 # Per-call silence gating
│       ├── requirements.txt        # emergent-client
│       └── tests/
│           └── test_audio_gate.py
├── scripts/
│   ├── setup-infer.sh              # govcraft: install whisper, piper, models
│   └── trigger_outbound.sh         # manually trigger outbound call
```

No Python scripts on govcraft — all inference steps are shell pipes in exec-handler args within `emergent-infer.toml`.

## Environment Variables

### penny-pi (.env)

| Variable | Description |
|---|---|
| `TWILIO_ACCOUNT_SID` | Twilio account SID |
| `TWILIO_AUTH_TOKEN` | Twilio auth token |
| `TWILIO_PHONE_NUMBER` | Twilio phone number (E.164) |
| `PUBLIC_URL` | Public Traefik URL for TwiML webhook (e.g., `https://penny-pi.example.com`) |
| `GOVCRAFT_HOST` | Tailscale IP or hostname for govcraft |

### govcraft (.env)

| Variable | Description |
|---|---|
| `OLLAMA_MODEL` | Ollama model name (e.g., `llama3.2`) |
| `WHISPER_MODEL` | Path to whisper ggml model file |
| `PIPER_MODEL` | Path to piper .onnx voice model |
| `PIPER_SAMPLE_RATE` | Output sample rate of piper model (e.g., `22050` for lessac-medium, `16000` for some others) |
| `PENNY_PI_HOST` | Tailscale IP or hostname for penny-pi (for return bridge) |

## Dependencies

### penny-pi

- Emergent engine (aarch64 binary)
- Python 3.12+, aiohttp, emergent-client (PyPI)
- Marketplace primitives: http-source, exec-handler, exec-sink
- Traefik (already running on host)

### govcraft

- Emergent engine (x86_64 binary)
- Marketplace primitives: http-source, exec-handler, exec-sink
- whisper.cpp (compiled with `GGML_CUDA=1`)
- Ollama (already running)
- piper TTS
- ffmpeg, jq, xxd (standard Unix tools)

## Latency Budget (Estimated)

| Step | Estimated Time |
|---|---|
| Audio gate (silence detection) | ~500ms (by design) |
| Bridge penny-pi → govcraft | ~5ms (Tailscale LAN) |
| ffmpeg decode + whisper STT | ~500ms–2s (GPU, depends on utterance length) |
| Ollama LLM generation | ~1–5s (depends on model and response length) |
| piper TTS + ffmpeg encode | ~200ms–1s (GPU) |
| Bridge govcraft → penny-pi | ~5ms |
| **Total per turn** | **~2–8s** |

## Error Handling

Each exec-handler publishes `exec.error` on non-zero exit, timeout, or spawn failure. Both engine console sinks subscribe to `exec.error` for visibility.

Pipeline failures are non-fatal to the caller's connection — the WebSocket stays open and the next utterance goes through normally. A failed turn simply produces no audio response (the caller hears silence until they speak again).

The twilio-bridge handler should log WebSocket disconnects and connection errors to stderr. It does not attempt reconnection — Twilio manages the WebSocket lifecycle.

## Out of Scope (v1)

- Conversation history / multi-turn context
- Streaming TTS (chunked audio as LLM generates tokens)
- Barge-in (interrupt Claude mid-response)
- Multiple concurrent calls (architecture supports it via stream_sid keying, but untested)
- Authentication on inter-engine bridge (Tailscale ACLs are sufficient for now)
- Monitoring / alerting
