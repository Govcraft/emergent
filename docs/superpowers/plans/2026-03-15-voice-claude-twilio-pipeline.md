# Voice-Claude Twilio Pipeline Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a turn-based phone pipeline so users can call Claude over the phone (and Claude can call users), using Emergent as the orchestration engine with fully local GPU-based STT and TTS.

**Architecture:** A custom Python handler (`twilio-bridge`) acts as both a TwiML HTTP endpoint (for Twilio call setup) and a WebSocket server (for Twilio Media Streams audio). Audio frames flow through a stateful `audio-gate` Python handler for silence-based turn detection, then through Python script exec-handlers for ffmpeg (μ-law decode), whisper-cli (STT on local GPU), claude CLI (LLM), piper (TTS on local GPU), and ffmpeg (re-encode to μ-law). The `twilio-bridge` sends the audio response back to Twilio over the same WebSocket. Outbound calls are triggered by publishing a `call.outbound.requested` Emergent event.

**Tech Stack:** Python 3.12, aiohttp, websockets, emergent-client (PyPI), Emergent engine v0.10.5, Twilio Media Streams, whisper.cpp (CUDA, compute 12.0), piper TTS, ffmpeg, claude CLI, Tailscale Funnel (public webhook URL)

---

## File Structure

### New Project: `/home/rodzilla/code/active/voice-claude/`

| File | Purpose |
|------|---------|
| `emergent.toml` | Full pipeline topology |
| `.env.example` | Template for required secrets |
| `.gitignore` | Exclude `.env`, models, `__pycache__`, tmp audio |
| `handlers/twilio-bridge/main.py` | Custom Python handler: HTTP TwiML server + WebSocket server, bridges Twilio ↔ Emergent |
| `handlers/twilio-bridge/requirements.txt` | aiohttp, websockets, emergent-client |
| `handlers/twilio-bridge/tests/test_twilio_bridge.py` | Unit tests for TwiML generation, frame parsing, media frame construction |
| `handlers/audio-gate/main.py` | Stateful Python handler: accumulate μ-law frames, silence threshold, flush utterance |
| `handlers/audio-gate/requirements.txt` | emergent-client |
| `handlers/audio-gate/tests/test_audio_gate.py` | Unit tests for silence detection, buffer accumulation, edge cases |
| `scripts/decode_audio.py` | Read JSON with mulaw_hex from stdin, write 16kHz WAV, output JSON with path |
| `scripts/encode_audio.py` | Read JSON with wav_path from stdin, encode to μ-law hex, output JSON |
| `scripts/setup.sh` | Install whisper.cpp (CUDA), piper, models, marketplace primitives |
| `scripts/trigger_outbound.sh` | Manually trigger an outbound call via Emergent HTTP API |

---

## Chunk 1: Project Scaffolding

### Task 1: Initialize project

**Files:**
- Create: `/home/rodzilla/code/active/voice-claude/` (directory)
- Create: `/home/rodzilla/code/active/voice-claude/.gitignore`
- Create: `/home/rodzilla/code/active/voice-claude/.env.example`

- [ ] **Step 1: Create project directory and initialize git**

```bash
mkdir -p /home/rodzilla/code/active/voice-claude
\cd /home/rodzilla/code/active/voice-claude
git init -b main
```

- [ ] **Step 2: Create .gitignore**

```
.env
models/
__pycache__/
*.pyc
*.wav
/tmp/
.pytest_cache/
```

- [ ] **Step 3: Create .env.example**

```bash
TWILIO_ACCOUNT_SID=ACxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
TWILIO_AUTH_TOKEN=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
TWILIO_PHONE_NUMBER=+1xxxxxxxxxx
PUBLIC_URL=https://your-machine.ts.net
CLAUDE_MODEL=claude-sonnet-4-6
WHISPER_MODEL=/home/rodzilla/code/active/voice-claude/models/whisper/ggml-large-v3.bin
PIPER_MODEL=/home/rodzilla/code/active/voice-claude/models/piper/en_US-lessac-medium.onnx
```

- [ ] **Step 4: Commit**

```bash
git add .gitignore .env.example
git commit -S -m "chore: initialize voice-claude project"
```

---

### Task 2: Install system dependencies

**Files:**
- Create: `scripts/setup.sh`

- [ ] **Step 1: Create setup script**

```bash
mkdir -p scripts
cat > scripts/setup.sh << 'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail

echo "=== whisper.cpp (CUDA) ==="
if ! command -v whisper-cli &>/dev/null; then
    git clone https://github.com/ggerganov/whisper.cpp /tmp/whisper-cpp
    \cd /tmp/whisper-cpp
    cmake -B build -DWHISPER_CUDA=ON
    cmake --build build --config Release -j$(nproc)
    sudo cp build/bin/whisper-cli /usr/local/bin/
    \cd /home/rodzilla/code/active/voice-claude
fi

echo "=== Whisper large-v3 model ==="
mkdir -p models/whisper
if [ ! -f models/whisper/ggml-large-v3.bin ]; then
    wget -q "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin" \
        -O models/whisper/ggml-large-v3.bin
fi

echo "=== piper TTS ==="
if ! command -v piper &>/dev/null; then
    wget -q "https://github.com/rhasspy/piper/releases/latest/download/piper_linux_x86_64.tar.gz" \
        -O /tmp/piper.tar.gz
    tar -xzf /tmp/piper.tar.gz -C /tmp/
    sudo cp /tmp/piper/piper /usr/local/bin/
fi

echo "=== Piper voice model (en_US-lessac-medium) ==="
mkdir -p models/piper
if [ ! -f models/piper/en_US-lessac-medium.onnx ]; then
    BASE="https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium"
    wget -q "$BASE/en_US-lessac-medium.onnx" -O models/piper/en_US-lessac-medium.onnx
    wget -q "$BASE/en_US-lessac-medium.onnx.json" -O models/piper/en_US-lessac-medium.onnx.json
fi

echo "=== Emergent marketplace primitives ==="
emergent marketplace install exec-handler exec-sink

echo "=== claude CLI ==="
if ! command -v claude &>/dev/null; then
    echo "ERROR: claude CLI not found. Install from https://claude.ai/code"
    exit 1
fi
claude --version

echo "=== Done ==="
SCRIPT
chmod +x scripts/setup.sh
```

- [ ] **Step 2: Run setup script**

```bash
source .env
bash scripts/setup.sh
```

Expected: All tools installed, model files present under `models/`.

- [ ] **Step 3: Verify installations**

```bash
whisper-cli --help 2>&1 | head -2
piper --help 2>&1 | head -2
ffmpeg -version 2>&1 | head -1
```

Expected: Each command outputs version/help text without error.

- [ ] **Step 4: Commit**

```bash
git add scripts/setup.sh
git commit -S -m "chore: add dependency setup script"
```

---

## Chunk 2: Audio Gate Handler

### Task 3: Implement audio-gate with TDD

**Files:**
- Create: `handlers/audio-gate/main.py`
- Create: `handlers/audio-gate/requirements.txt`
- Create: `handlers/audio-gate/tests/__init__.py`
- Create: `handlers/audio-gate/tests/test_audio_gate.py`

- [ ] **Step 1: Create directories and requirements**

```bash
mkdir -p handlers/audio-gate/tests
touch handlers/audio-gate/__init__.py
touch handlers/audio-gate/tests/__init__.py

cat > handlers/audio-gate/requirements.txt << 'EOF'
emergent-client>=0.1.0
pytest>=8.0
pytest-asyncio>=0.23
EOF

pip install -r handlers/audio-gate/requirements.txt
```

- [ ] **Step 2: Write failing tests**

```python
# handlers/audio-gate/tests/test_audio_gate.py
import sys
sys.path.insert(0, "handlers/audio-gate")

from main import AudioGate, SILENCE_THRESHOLD, SILENCE_FRAMES_REQUIRED, MIN_SPEECH_FRAMES


def _silence(n: int = 160) -> bytes:
    """μ-law silence bytes cluster near 0xFF."""
    return bytes([0xFF] * n)


def _speech(n: int = 160) -> bytes:
    """μ-law speech bytes are far from 0xFF."""
    return bytes([0x40] * n)


class TestAudioGate:
    def setup_method(self):
        self.gate = AudioGate()

    def test_accumulates_frames(self):
        self.gate.push_frame(_speech())
        assert len(self.gate.buffer) == 160

    def test_returns_none_during_speech(self):
        for _ in range(50):
            assert self.gate.push_frame(_speech()) is None

    def test_flush_after_required_silence_frames(self):
        for _ in range(MIN_SPEECH_FRAMES + 1):
            self.gate.push_frame(_speech())
        result = None
        for _ in range(SILENCE_FRAMES_REQUIRED + 1):
            result = self.gate.push_frame(_silence())
        assert result is not None
        assert len(result) > 0

    def test_resets_buffer_after_flush(self):
        for _ in range(MIN_SPEECH_FRAMES + 1):
            self.gate.push_frame(_speech())
        for _ in range(SILENCE_FRAMES_REQUIRED + 1):
            self.gate.push_frame(_silence())
        assert len(self.gate.buffer) == 0
        assert self.gate.silence_frames == 0
        assert self.gate.speech_frames == 0

    def test_silence_detection_near_0xff(self):
        assert AudioGate.is_silent(bytes([0xFF] * 160))

    def test_speech_detection_far_from_0xff(self):
        assert not AudioGate.is_silent(bytes([0x20] * 160))

    def test_no_flush_on_empty_buffer(self):
        """Discard noise bursts — don't flush if speech threshold not met."""
        result = None
        for _ in range(SILENCE_FRAMES_REQUIRED + 1):
            result = self.gate.push_frame(_silence())
        assert result is None

    def test_accumulated_bytes_match_input(self):
        frame = _speech(320)
        self.gate.push_frame(frame)
        assert self.gate.buffer == bytearray(frame)
```

- [ ] **Step 3: Run tests — confirm they fail**

```bash
\cd /home/rodzilla/code/active/voice-claude
python -m pytest handlers/audio-gate/tests/test_audio_gate.py -v
```

Expected: `ModuleNotFoundError` — no implementation yet.

- [ ] **Step 4: Write minimal implementation**

```python
# handlers/audio-gate/main.py
import asyncio
from emergent import run_handler, create_message

SILENCE_THRESHOLD = 240       # μ-law byte avg above this = silence (near 0xFF)
SILENCE_FRAMES_REQUIRED = 25  # ~500ms at 20ms Twilio frames
MIN_SPEECH_FRAMES = 5         # discard noise bursts shorter than this


class AudioGate:
    def __init__(self):
        self.buffer = bytearray()
        self.silence_frames = 0
        self.speech_frames = 0

    @staticmethod
    def is_silent(frame: bytes) -> bool:
        if not frame:
            return True
        return (sum(frame) / len(frame)) > SILENCE_THRESHOLD

    def push_frame(self, frame: bytes) -> bytes | None:
        """Add a frame. Returns flushed buffer when end-of-turn is detected."""
        self.buffer.extend(frame)
        if self.is_silent(frame):
            self.silence_frames += 1
        else:
            self.silence_frames = 0
            self.speech_frames += 1

        if self.silence_frames >= SILENCE_FRAMES_REQUIRED:
            if self.speech_frames >= MIN_SPEECH_FRAMES:
                result = bytes(self.buffer)
                self.buffer = bytearray()
                self.silence_frames = 0
                self.speech_frames = 0
                return result
            # Noise burst — discard
            self.buffer = bytearray()
            self.silence_frames = 0
            self.speech_frames = 0

        return None


_gate = AudioGate()


async def process(msg, handler):
    raw = bytes.fromhex(msg.payload.get("mulaw_hex", ""))
    if not raw:
        return
    flushed = _gate.push_frame(raw)
    if flushed is not None:
        await handler.publish(
            create_message("audio.utterance")
            .caused_by(msg.id)
            .payload({
                "mulaw_hex": flushed.hex(),
                "stream_sid": msg.payload.get("stream_sid", ""),
            })
        )


if __name__ == "__main__":
    asyncio.run(run_handler("audio-gate", ["audio.frame"], process))
```

- [ ] **Step 5: Run tests — confirm they pass**

```bash
python -m pytest handlers/audio-gate/tests/test_audio_gate.py -v
```

Expected: All 8 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add handlers/audio-gate/
git commit -S -m "feat: add stateful audio-gate handler with silence detection"
```

---

## Chunk 3: Twilio Bridge Handler

### Task 4: Implement twilio-bridge with TDD

**Files:**
- Create: `handlers/twilio-bridge/main.py`
- Create: `handlers/twilio-bridge/requirements.txt`
- Create: `handlers/twilio-bridge/tests/__init__.py`
- Create: `handlers/twilio-bridge/tests/test_twilio_bridge.py`

- [ ] **Step 1: Create directories and requirements**

```bash
mkdir -p handlers/twilio-bridge/tests
touch handlers/twilio-bridge/__init__.py
touch handlers/twilio-bridge/tests/__init__.py

cat > handlers/twilio-bridge/requirements.txt << 'EOF'
emergent-client>=0.1.0
aiohttp>=3.9
websockets>=12.0
pytest>=8.0
pytest-asyncio>=0.23
EOF

pip install -r handlers/twilio-bridge/requirements.txt
```

- [ ] **Step 2: Write failing tests**

```python
# handlers/twilio-bridge/tests/test_twilio_bridge.py
import json
import sys
import xml.etree.ElementTree as ET
sys.path.insert(0, "handlers/twilio-bridge")

from main import build_twiml_response, parse_twilio_frame, build_twilio_media_frame


class TestTwimlResponse:
    def test_response_root_element(self):
        root = ET.fromstring(build_twiml_response("wss://example.ts.net/ws"))
        assert root.tag == "Response"

    def test_contains_connect_with_stream(self):
        xml = build_twiml_response("wss://example.ts.net/ws")
        assert "Connect" in xml
        assert "Stream" in xml

    def test_url_is_embedded(self):
        xml = build_twiml_response("wss://example.ts.net/ws")
        assert "wss://example.ts.net/ws" in xml


class TestParseTwilioFrame:
    def test_parses_media_event(self):
        raw = json.dumps({
            "event": "media",
            "streamSid": "MX123",
            "media": {"payload": "AAEC"},
        })
        result = parse_twilio_frame(raw)
        assert result is not None
        assert result["stream_sid"] == "MX123"
        assert result["payload"] == "AAEC"

    def test_ignores_connected_event(self):
        raw = json.dumps({"event": "connected", "protocol": "Call"})
        assert parse_twilio_frame(raw) is None

    def test_ignores_start_event(self):
        raw = json.dumps({"event": "start", "streamSid": "MX123"})
        assert parse_twilio_frame(raw) is None

    def test_ignores_stop_event(self):
        raw = json.dumps({"event": "stop", "streamSid": "MX123"})
        assert parse_twilio_frame(raw) is None

    def test_handles_malformed_json(self):
        assert parse_twilio_frame("not json {{{") is None


class TestBuildTwilioMediaFrame:
    def test_event_type(self):
        data = json.loads(build_twilio_media_frame("MX123", b"\xff\x00"))
        assert data["event"] == "media"

    def test_stream_sid(self):
        data = json.loads(build_twilio_media_frame("MX123", b"\xff\x00"))
        assert data["streamSid"] == "MX123"

    def test_payload_is_base64(self):
        import base64
        data = json.loads(build_twilio_media_frame("MX123", b"\xff\x00"))
        decoded = base64.b64decode(data["media"]["payload"])
        assert decoded == b"\xff\x00"
```

- [ ] **Step 3: Run tests — confirm they fail**

```bash
python -m pytest handlers/twilio-bridge/tests/test_twilio_bridge.py -v
```

Expected: `ImportError` — no implementation yet.

- [ ] **Step 4: Write minimal implementation**

```python
# handlers/twilio-bridge/main.py
import asyncio
import base64
import json
import os
from typing import Optional

import aiohttp.web
import websockets
import websockets.server
from emergent import EmergentHandler, create_message


def build_twiml_response(ws_url: str) -> str:
    return (
        '<?xml version="1.0" encoding="UTF-8"?>'
        "<Response><Connect>"
        f'<Stream url="{ws_url}" />'
        "</Connect></Response>"
    )


def parse_twilio_frame(raw: str) -> Optional[dict]:
    try:
        data = json.loads(raw)
        if data.get("event") != "media":
            return None
        return {
            "stream_sid": data.get("streamSid", ""),
            "payload": data["media"]["payload"],
        }
    except (json.JSONDecodeError, KeyError, TypeError):
        return None


def build_twilio_media_frame(stream_sid: str, audio_bytes: bytes) -> str:
    return json.dumps({
        "event": "media",
        "streamSid": stream_sid,
        "media": {"payload": base64.b64encode(audio_bytes).decode()},
    })


# Active WebSocket connections keyed by stream_sid
_connections: dict[str, websockets.server.WebSocketServerProtocol] = {}
_emergent_handler: Optional[EmergentHandler] = None


async def _handle_twilio_ws(websocket):
    stream_sid = None
    try:
        async for raw in websocket:
            frame = parse_twilio_frame(raw)
            if frame is None:
                continue
            stream_sid = frame["stream_sid"]
            _connections[stream_sid] = websocket
            if _emergent_handler:
                mulaw = base64.b64decode(frame["payload"])
                await _emergent_handler.publish(
                    create_message("audio.frame").payload({
                        "mulaw_hex": mulaw.hex(),
                        "stream_sid": stream_sid,
                    })
                )
    finally:
        if stream_sid:
            _connections.pop(stream_sid, None)


async def _handle_twiml_webhook(request: aiohttp.web.Request) -> aiohttp.web.Response:
    public_url = os.environ["PUBLIC_URL"].rstrip("/")
    # Strip scheme so we can prepend wss://
    host = public_url.split("://", 1)[-1]
    ws_url = f"wss://{host}/twilio-ws"
    return aiohttp.web.Response(
        text=build_twiml_response(ws_url),
        content_type="text/xml",
    )


async def _send_audio_to_twilio(msg):
    stream_sid = msg.payload.get("stream_sid", "")
    mulaw_hex = msg.payload.get("mulaw_hex", "")
    if not stream_sid or not mulaw_hex:
        return
    ws = _connections.get(stream_sid)
    if ws:
        frame = build_twilio_media_frame(stream_sid, bytes.fromhex(mulaw_hex))
        await ws.send(frame)


async def main():
    global _emergent_handler
    handler = await EmergentHandler.connect("twilio-bridge")
    _emergent_handler = handler
    await handler.subscribe(["audio.out"])

    # HTTP server for TwiML webhook
    app = aiohttp.web.Application()
    app.router.add_post("/voice", _handle_twiml_webhook)
    runner = aiohttp.web.AppRunner(app)
    await runner.setup()
    await aiohttp.web.TCPSite(runner, "0.0.0.0", 8080).start()

    # WebSocket server for Twilio audio stream
    ws_server = await websockets.serve(_handle_twilio_ws, "0.0.0.0", 8081)

    print("TwiML webhook:  http://0.0.0.0:8080/voice", flush=True)
    print("Twilio WS:      ws://0.0.0.0:8081/twilio-ws", flush=True)

    # Forward audio.out messages to the correct Twilio WebSocket
    async for msg in handler.stream():
        await _send_audio_to_twilio(msg)


if __name__ == "__main__":
    asyncio.run(main())
```

- [ ] **Step 5: Run tests — confirm they pass**

```bash
python -m pytest handlers/twilio-bridge/tests/test_twilio_bridge.py -v
```

Expected: All 11 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add handlers/twilio-bridge/
git commit -S -m "feat: add twilio-bridge handler with TwiML + WebSocket server"
```

---

## Chunk 4: Audio Processing Scripts

### Task 5: Decode script (μ-law → WAV)

**Files:**
- Create: `scripts/decode_audio.py`

- [ ] **Step 1: Write decode_audio.py**

Reads JSON with `mulaw_hex` and `stream_sid` from stdin. Writes a 16kHz mono WAV file to `/tmp/{stream_sid}.wav`. Outputs JSON with `wav_path` and `stream_sid`.

```python
#!/usr/bin/env python3
# scripts/decode_audio.py
import json
import subprocess
import sys
import tempfile
import os

data = json.load(sys.stdin)
mulaw_hex = data["mulaw_hex"]
stream_sid = data["stream_sid"]

# Write raw μ-law bytes to a temp file
raw_path = f"/tmp/{stream_sid}.mulaw"
wav_path = f"/tmp/{stream_sid}.wav"

with open(raw_path, "wb") as f:
    f.write(bytes.fromhex(mulaw_hex))

subprocess.run(
    [
        "ffmpeg", "-y", "-loglevel", "quiet",
        "-f", "mulaw", "-ar", "8000", "-ac", "1", "-i", raw_path,
        "-ar", "16000", "-ac", "1", wav_path,
    ],
    check=True,
)

os.unlink(raw_path)
json.dump({"wav_path": wav_path, "stream_sid": stream_sid}, sys.stdout)
```

- [ ] **Step 2: Make executable and test manually**

```bash
chmod +x scripts/decode_audio.py
# Quick smoke test with silence bytes
python3 -c "import json; print(json.dumps({'mulaw_hex': 'ff' * 160, 'stream_sid': 'test'}))" \
    | python3 scripts/decode_audio.py
```

Expected: JSON output with `wav_path: /tmp/test.wav`.

- [ ] **Step 3: Commit**

```bash
git add scripts/decode_audio.py
git commit -S -m "feat: add μ-law to WAV decode script"
```

---

### Task 6: Encode script (WAV → μ-law)

**Files:**
- Create: `scripts/encode_audio.py`

- [ ] **Step 1: Write encode_audio.py**

Reads JSON with `wav_path` and `stream_sid` from stdin. Encodes WAV to μ-law 8kHz. Outputs JSON with `mulaw_hex` and `stream_sid`.

```python
#!/usr/bin/env python3
# scripts/encode_audio.py
import json
import subprocess
import sys
import os

data = json.load(sys.stdin)
wav_path = data["wav_path"]
stream_sid = data["stream_sid"]

mulaw_path = f"/tmp/{stream_sid}_out.mulaw"

subprocess.run(
    [
        "ffmpeg", "-y", "-loglevel", "quiet",
        "-i", wav_path,
        "-f", "mulaw", "-ar", "8000", "-ac", "1", mulaw_path,
    ],
    check=True,
)

with open(mulaw_path, "rb") as f:
    mulaw_bytes = f.read()

os.unlink(mulaw_path)
if os.path.exists(wav_path):
    os.unlink(wav_path)

json.dump({"mulaw_hex": mulaw_bytes.hex(), "stream_sid": stream_sid}, sys.stdout)
```

- [ ] **Step 2: Make executable and smoke test**

```bash
chmod +x scripts/encode_audio.py
# Round-trip test: decode then re-encode
python3 -c "import json; print(json.dumps({'mulaw_hex': 'ff' * 160, 'stream_sid': 'test'}))" \
    | python3 scripts/decode_audio.py \
    | python3 scripts/encode_audio.py
```

Expected: JSON output with `mulaw_hex` and `stream_sid: test`.

- [ ] **Step 3: Commit**

```bash
git add scripts/encode_audio.py
git commit -S -m "feat: add WAV to μ-law encode script"
```

---

## Chunk 5: Pipeline Configuration

### Task 7: Write emergent.toml

**Files:**
- Create: `/home/rodzilla/code/active/voice-claude/emergent.toml`

- [ ] **Step 1: Write emergent.toml**

```toml
[engine]
name = "voice-claude"
socket_path = "auto"
api_port = 8090

# =============================================================================
# twilio-bridge: HTTP TwiML webhook (port 8080) + WebSocket server (port 8081)
# Publishes: audio.frame  (Twilio audio in, μ-law hex + stream_sid)
# Subscribes: audio.out   (TTS μ-law hex + stream_sid, send back to Twilio)
# =============================================================================
[[handlers]]
name = "twilio-bridge"
path = "python3"
args = ["handlers/twilio-bridge/main.py"]
subscribes = ["audio.out"]
publishes = ["audio.frame"]

# =============================================================================
# audio-gate: stateful silence detection gate
# Subscribes: audio.frame  — accumulates μ-law frames
# Publishes:  audio.utterance — complete utterance on silence
# =============================================================================
[[handlers]]
name = "audio-gate"
path = "python3"
args = ["handlers/audio-gate/main.py"]
subscribes = ["audio.frame"]
publishes = ["audio.utterance"]

# =============================================================================
# decode-audio: μ-law 8kHz → 16kHz WAV via ffmpeg
# =============================================================================
[[handlers]]
name = "decode-audio"
path = "python3"
args = ["scripts/decode_audio.py"]
subscribes = ["audio.utterance"]
publishes = ["audio.wav"]

# =============================================================================
# transcribe: whisper-cli STT with CUDA
# Reads wav_path, outputs { text, stream_sid }
# =============================================================================
[[handlers]]
name = "transcribe"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "audio.wav",
    "--publish-as", "audio.transcript",
    "--timeout", "30000",
    "--",
    "sh", "-c",
    """input=$(cat) \
    && path=$(echo "$input" | jq -r .wav_path) \
    && sid=$(echo "$input" | jq -r .stream_sid) \
    && text=$(whisper-cli -m "$WHISPER_MODEL" -f "$path" --output-txt --no-timestamps -l en 2>/dev/null | grep -v '^\[' | xargs) \
    && rm -f "$path" \
    && jq -n --arg t "$text" --arg s "$sid" '{text: $t, stream_sid: $s}'"""
]
subscribes = ["audio.wav"]
publishes = ["audio.transcript"]

# =============================================================================
# claude-respond: send transcript to claude CLI, get response
# =============================================================================
[[handlers]]
name = "claude-respond"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "audio.transcript",
    "--publish-as", "claude.response",
    "--timeout", "60000",
    "--",
    "sh", "-c",
    """input=$(cat) \
    && text=$(echo "$input" | jq -r .text) \
    && sid=$(echo "$input" | jq -r .stream_sid) \
    && response=$(printf '%s' "$text" | claude --model "$CLAUDE_MODEL" \
        -p 'You are a helpful voice assistant. Be concise — 1 to 3 sentences max.' 2>/dev/null) \
    && jq -n --arg r "$response" --arg s "$sid" '{text: $r, stream_sid: $s}'"""
]
subscribes = ["audio.transcript"]
publishes = ["claude.response"]

# =============================================================================
# synthesize: piper TTS, text → WAV
# =============================================================================
[[handlers]]
name = "synthesize"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "claude.response",
    "--publish-as", "tts.wav",
    "--timeout", "30000",
    "--",
    "sh", "-c",
    """input=$(cat) \
    && text=$(echo "$input" | jq -r .text) \
    && sid=$(echo "$input" | jq -r .stream_sid) \
    && out="/tmp/${sid}_tts.wav" \
    && printf '%s' "$text" | piper --model "$PIPER_MODEL" --output_file "$out" 2>/dev/null \
    && jq -n --arg p "$out" --arg s "$sid" '{wav_path: $p, stream_sid: $s}'"""
]
subscribes = ["claude.response"]
publishes = ["tts.wav"]

# =============================================================================
# encode-audio: WAV → μ-law 8kHz for Twilio
# =============================================================================
[[handlers]]
name = "encode-audio"
path = "python3"
args = ["scripts/encode_audio.py"]
subscribes = ["tts.wav"]
publishes = ["audio.out"]

# =============================================================================
# outbound-caller: trigger an outbound call via Twilio REST API
# Publish { "to": "+1xxxxxxxxxx" } to call.outbound.requested to trigger
# =============================================================================
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

# =============================================================================
# console: debug sink — log transcripts and Claude responses
# =============================================================================
[[sinks]]
name = "console"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = [
    "-s", "audio.transcript,claude.response,system.error.*",
    "--", "jq", "-c", "."
]
subscribes = ["audio.transcript", "claude.response", "system.error.*"]
```

- [ ] **Step 2: Validate TOML syntax**

```bash
python3 -c "import tomllib; tomllib.load(open('emergent.toml','rb')); print('TOML OK')"
```

Expected: `TOML OK`

- [ ] **Step 3: Commit**

```bash
git add emergent.toml
git commit -S -m "feat: add emergent.toml pipeline configuration"
```

---

## Chunk 6: Infrastructure & End-to-End

### Task 8: Tailscale Funnel + Twilio webhook configuration

- [ ] **Step 1: Enable Tailscale Funnel for port 8080 (TwiML webhook)**

```bash
tailscale funnel 8080
```

Expected: Output shows your public HTTPS URL (e.g., `https://your-machine.ts.net`).

- [ ] **Step 2: Also expose port 8081 for WebSocket**

```bash
tailscale funnel 8081
```

Note: Tailscale Funnel automatically proxies WSS to WS internally.

- [ ] **Step 3: Copy and fill in .env**

```bash
cp .env.example .env
# Edit .env with real values:
#   PUBLIC_URL=https://your-machine.ts.net
#   TWILIO_ACCOUNT_SID=ACxxx
#   TWILIO_AUTH_TOKEN=xxx
#   TWILIO_PHONE_NUMBER=+1xxx
```

- [ ] **Step 4: Configure Twilio phone number**

In Twilio Console → Phone Numbers → Active Numbers → your number:
- A call comes in → Webhook: `POST https://your-machine.ts.net/voice`
- Save.

---

### Task 9: End-to-end smoke test

- [ ] **Step 1: Source environment**

```bash
source .env
```

- [ ] **Step 2: Start Emergent**

```bash
\cd /home/rodzilla/code/active/voice-claude
emergent --config emergent.toml
```

Expected: Engine starts, `system.started.*` events appear for all handlers.

- [ ] **Step 3: Verify TwiML webhook is reachable**

```bash
curl -X POST "$PUBLIC_URL/voice"
```

Expected: XML response containing `<Stream`.

- [ ] **Step 4: Place test inbound call**

Call `$TWILIO_PHONE_NUMBER` from any phone. Speak a short phrase after the line connects. Verify in console:
1. `audio.transcript` event appears with your spoken words
2. `claude.response` event appears with Claude's reply
3. You hear Claude's voice through the phone

- [ ] **Step 5: Test outbound call trigger**

```bash
cat > scripts/trigger_outbound.sh << 'EOF'
#!/usr/bin/env bash
TO="${1:?Usage: trigger_outbound.sh +1xxxxxxxxxx}"
source "$(dirname "$0")/../.env"
curl -s -X POST "http://localhost:8090/publish" \
    -H "Content-Type: application/json" \
    -d "{\"message_type\": \"call.outbound.requested\", \"payload\": {\"to\": \"$TO\"}}"
EOF
chmod +x scripts/trigger_outbound.sh

bash scripts/trigger_outbound.sh +1YOUR_PHONE
```

Expected: Your phone rings. On answer, the same pipeline activates.

- [ ] **Step 6: Final commit**

```bash
git add scripts/trigger_outbound.sh
git commit -S -m "feat: add outbound call trigger script"
```

---

## Environment Variables Reference

| Variable | Description |
|---|---|
| `TWILIO_ACCOUNT_SID` | Twilio account SID (starts with `AC`) |
| `TWILIO_AUTH_TOKEN` | Twilio auth token |
| `TWILIO_PHONE_NUMBER` | Your Twilio phone number (E.164, e.g., `+15551234567`) |
| `PUBLIC_URL` | Tailscale Funnel URL (e.g., `https://your-machine.ts.net`) |
| `CLAUDE_MODEL` | Claude model ID (e.g., `claude-sonnet-4-6`) |
| `WHISPER_MODEL` | Absolute path to whisper ggml model file |
| `PIPER_MODEL` | Absolute path to piper `.onnx` voice model file |

## Pipeline Message Flow Reference

```
Inbound call:
  Twilio (phone call)
    → POST /voice (HTTP)        twilio-bridge publishes: audio.frame
    → WS /twilio-ws (WebSocket) twilio-bridge publishes: audio.frame per chunk
    → audio-gate                buffers + silence detect → audio.utterance
    → decode-audio              μ-law 8kHz → 16kHz WAV  → audio.wav
    → transcribe                whisper-cli (GPU)        → audio.transcript
    → claude-respond            claude CLI               → claude.response
    → synthesize                piper (GPU)              → tts.wav
    → encode-audio              WAV → μ-law 8kHz         → audio.out
    → twilio-bridge             WebSocket send to Twilio → caller hears response

Outbound call:
  Emergent event: call.outbound.requested { "to": "+1xxx" }
    → outbound-caller sink  → Twilio REST API → phone rings
    → on answer: same pipeline as inbound
```
