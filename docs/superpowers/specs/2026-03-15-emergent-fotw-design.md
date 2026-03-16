# emergent-fotw: Fly on the Wall

*Meeting transcription pipeline using local AI inference with Emergent.*

**Project directory**: `~/code/active/emergent-fotw`

**Run from project directory**: All relative paths in the config (scripts, transcript output) assume the engine is started from `~/code/active/emergent-fotw/`.

---

## Purpose

A multi-stream audio transcription tool built as an Emergent pipeline. Captures audio from multiple sources (local microphone, speaker output, remote microphones), transcribes locally using whisper.cpp with CUDA acceleration, and outputs speaker-attributed transcripts to both console and file. Designed for meeting/conversation recording where accuracy matters more than latency.

---

## Machine Context

| Component | Details |
|-----------|---------|
| CPU | Intel Core Ultra 9 275HX — 24 cores, 6.5 GHz boost |
| RAM | 128 GB |
| GPU | NVIDIA RTX PRO 5000 Black — 24 GB VRAM, CUDA 13.1 |
| OS | Arch Linux 6.19.6, PipeWire audio |

The GPU runs whisper.cpp `large-v3` with CUDA acceleration. Model loading takes ~3-5 seconds per invocation; inference on a typical VAD chunk (2-10 seconds of speech) takes ~1-2 seconds on this hardware.

---

## Pipeline Topology

```
source "mic-local"     (audio_capture.py, SDK source, AUDIO_DEVICE=..., SPEAKER_LABEL="me")        ──┐
source "speakers"      (audio_capture.py, SDK source, AUDIO_DEVICE=..., SPEAKER_LABEL="remote")    ──┼─→ exec-handler "transcribe" (whisper.cpp whisper-cli -f -)
source "mic-remote"    (audio_capture.py, SDK source, AUDIO_DEVICE=..., SPEAKER_LABEL="guest")     ──┘       ├─→ exec-sink "console" (format_transcript.sh)
                                                                                                               └─→ exec-sink "transcript" (format_transcript.sh >> file)
```

No daemon. No server. No lifecycle management. whisper.cpp reads stdin and writes stdout — pure Unix composability through Emergent's exec-handler.

### Primitive types

- **SDK source** `mic-*`, `speakers`: Custom Python sources using the Emergent Python SDK (`audio_capture.py`). Long-lived processes that connect to the engine via IPC and publish messages directly. These are NOT exec-sources — they need continuous audio capture with VAD state across messages.
- **exec-handler** `transcribe`: marketplace primitive, stateless — pipes audio through whisper.cpp per message. Model loads per invocation.
- **exec-sink** `console`, `transcript`: marketplace primitives, stateless output

All are declared using the standard `[[sources]]`, `[[handlers]]`, `[[sinks]]` TOML tables. The distinction is in what binary they run (SDK script vs marketplace exec binary).

### Message Flow

```
audio.chunk  →  transcript.segment  →  console + file
```

1. Audio sources capture from PipeWire devices, detect speech boundaries via VAD, write audio chunks to temp WAV files, emit `audio.chunk` events with the file path + speaker label
2. Transcription handler receives `audio.chunk`, pipes the WAV file through whisper.cpp (`whisper-cli -f -`), wraps the plain text output with speaker attribution, publishes `transcript.segment`
3. Sinks consume `transcript.segment` — console prints live, file sink appends to a dated transcript

### Data handoff: file paths, not embedded audio

Audio data is passed between primitives as file paths, not base64-encoded blobs. Each audio source writes its VAD-detected speech chunk to a temporary WAV file in `$XDG_RUNTIME_DIR/emergent-fotw/` and publishes the file path in the message payload. The transcription handler reads the file, feeds it to whisper.cpp via stdin, and deletes it after transcription.

This avoids:
- Bloating the event store with binary audio data
- Shell variable size limits in exec-handler
- Unnecessary base64 encode/decode overhead

The event store logs only lightweight JSON (file paths, speaker labels, transcript text) — not audio.

### Throughput and latency

whisper.cpp loads the model on every invocation (~3-5 seconds) plus inference time (~1-2 seconds per chunk). Total: ~5-7 seconds per chunk.

| Sources | Chunks/min (est.) | Processing/chunk | Keeps up with real-time? |
|---------|-------------------|-----------------|--------------------------|
| 1 mic | ~3-4 | ~5-7s | Yes — ~25s processing per 60s audio |
| 3 mics | ~10-12 | ~5-7s each | Borderline — may lag behind live speech |

For a meeting recorder, lagging behind live speech is acceptable. Emergent's pub-sub buffers the message queue. The transcript catches up after the meeting ends or during silences.

**Upgrade path if throughput becomes a problem:** whisper.cpp ships with a built-in HTTP server mode (`whisper-server` binary). Swap the exec-handler for a `curl localhost` call hitting the whisper.cpp server — model loads once, inference only. The audio sources and sinks don't change. This is a one-line TOML change.

---

## Components

### 1. Audio Capture Source (`audio_capture.py`)

A custom Python source using the Emergent Python SDK. Captures audio from a PipeWire/PulseAudio device and emits chunks on speech boundaries. This is a long-lived process that connects to the engine via IPC — not an exec-source.

**Responsibilities:**
- Open an audio input stream on the device specified by `AUDIO_DEVICE` env var
- Run Silero VAD (ONNX variant) to detect speech start/end boundaries
- When a speech segment ends (silence threshold crossed), write the audio to a temp WAV file and publish an `audio.chunk` event with the file path
- Tag each chunk with the `SPEAKER_LABEL` env var for attribution
- Create temp directory `$XDG_RUNTIME_DIR/emergent-fotw/` on startup

**Configuration via environment:**

| Variable | Purpose | Example |
|----------|---------|---------|
| `AUDIO_DEVICE` | PipeWire/PulseAudio source name | `alsa_input.usb-Blue_Yeti-00.analog-stereo` |
| `SPEAKER_LABEL` | Human-readable speaker name | `"me"`, `"remote"`, `"guest"` |
| `VAD_THRESHOLD` | Silero VAD confidence threshold (optional, default 0.5) | `0.3` |
| `SILENCE_DURATION_MS` | Silence duration to trigger chunk boundary (optional, default 1500) | `2000` |
| `SAMPLE_RATE` | Audio sample rate (optional, default 16000) | `16000` |

**VAD buffering algorithm:**

```
State: IDLE or RECORDING
Ring buffer: 500ms pre-roll (captures speech onset before VAD triggers)
Accumulator: growing buffer of audio frames during RECORDING state

1. Continuously read 30ms audio frames from sounddevice callback
2. Run each frame through Silero VAD → speech probability (0.0-1.0)
3. State machine:
   - IDLE + probability >= VAD_THRESHOLD:
       → transition to RECORDING
       → prepend ring buffer contents to accumulator (captures word onset)
   - RECORDING + probability >= VAD_THRESHOLD:
       → append frame to accumulator
       → reset silence counter
   - RECORDING + probability < VAD_THRESHOLD:
       → append frame to accumulator
       → increment silence counter
       → if silence counter >= SILENCE_DURATION_MS:
           → write accumulator to WAV file
           → publish audio.chunk with file path
           → clear accumulator, transition to IDLE
   - IDLE + probability < VAD_THRESHOLD:
       → push frame to ring buffer (FIFO, capped at 500ms)
4. On shutdown signal: if RECORDING, flush accumulator as final chunk
```

The ring buffer pre-roll ensures the first syllable of speech is not clipped. The silence duration threshold prevents splitting mid-pause within a single utterance.

**Published message (`audio.chunk`):**

```json
{
  "audio_path": "/run/user/1000/emergent-fotw/chunk_1710350630000_mic-local.wav",
  "speaker": "me",
  "duration_ms": 4200,
  "sample_rate": 16000
}
```

**Technology:** Python, sounddevice (PortAudio bindings), Silero VAD ONNX model (`silero_vad.onnx` — downloaded once, ~2 MB), onnxruntime, emergent-client SDK. Run via `uv`.

**Silero VAD ONNX model:** Downloaded on first run from the Silero GitHub releases. Cached locally in `$XDG_DATA_HOME/silero-vad/silero_vad.onnx`. The `onnxruntime` package runs inference without requiring PyTorch.

**Audio format:** 16kHz, mono, 16-bit signed PCM WAV. This is what whisper.cpp expects — no resampling needed.

### 2. Transcription Handler (`transcribe.sh` via exec-handler)

A marketplace exec-handler that runs `transcribe.sh` for each `audio.chunk` message. The script pipes the WAV file through whisper.cpp, wraps the output with speaker attribution and timestamp, and cleans up the temp file. Configured with a 30-second timeout (`--timeout 30000`) — sufficient for `large-v3` on GPU for typical speech segments. Tune higher if transcribing very long monologues.

**`transcribe.sh`:**

```bash
#!/bin/sh
set -e

WHISPER_MODEL="${WHISPER_MODEL:-${HOME}/.local/share/whisper.cpp/models/ggml-large-v3.bin}"
WHISPER_BIN="${WHISPER_BIN:-whisper-cli}"

# Read payload from stdin
input=$(cat)
speaker=$(echo "$input" | jq -r .speaker)
audio_path=$(echo "$input" | jq -r .audio_path)
timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Validate audio file exists
[ -f "$audio_path" ] || {
  echo "audio file not found: $audio_path" >&2
  exit 1
}

# Transcribe: pipe WAV through whisper.cpp, capture plain text output
text=$(cat "$audio_path" | "$WHISPER_BIN" -m "$WHISPER_MODEL" -f - --no-timestamps -t 4 2>/dev/null) || {
  echo "whisper.cpp failed for $audio_path" >&2
  exit 1
}

# Clean up temp audio file
rm -f "$audio_path"

# Emit transcript segment as JSON
jq -nc \
  --arg speaker "$speaker" \
  --arg text "$text" \
  --arg ts "$timestamp" \
  '{speaker: $speaker, text: $text, timestamp_utc: $ts}'
```

Key flags:
- `-f -`: read audio from stdin
- `--no-timestamps`: plain text output (no `[00:00.000 --> 00:02.400]` prefixes)
- `-t 4`: use 4 CPU threads for the non-GPU portions of inference

When the script exits non-zero, exec-handler does not publish `transcript.segment`. The audio file is preserved for debugging; orphaned temp files accumulate in `$XDG_RUNTIME_DIR/emergent-fotw/` (tmpfs, cleared on reboot).

**Published message (`transcript.segment`):**

```json
{
  "speaker": "me",
  "text": "Hello everyone. Let's get started.",
  "timestamp_utc": "2026-03-15T20:30:00Z"
}
```

Note: the output is plain concatenated text. whisper.cpp with `--no-timestamps` emits a single text block rather than per-segment timestamps. If per-segment timing is needed later, remove `--no-timestamps` and parse whisper.cpp's timestamped output in the script.

### 3. Transcript Formatter (`format_transcript.sh`)

Shared script used by both sinks to format `transcript.segment` into readable lines:

```bash
#!/bin/sh
# Read payload from stdin, format as a transcript line
jq -r '"[\(.timestamp_utc)] \(.speaker): \(.text)"'
```

Output:
```
[2026-03-15T20:30:00Z] me: Hello everyone. Let's get started.
[2026-03-15T20:30:05Z] remote: Thanks, let me share my screen.
```

### 4. Console Sink (exec-sink)

Marketplace exec-sink piping through `format_transcript.sh` for live display.

### 5. File Sink (exec-sink)

Marketplace exec-sink piping through `format_transcript.sh`, appending to a dated transcript file. The `$(date +%Y-%m-%d)` in the default path is evaluated per-message; if a meeting crosses midnight, the file name changes. Set `FOTW_TRANSCRIPT_FILE` to a fixed path to avoid this.

---

## Configuration

Run from the project directory:
```bash
cd ~/code/active/emergent-fotw
emergent --config ./emergent.toml
```

```toml
[engine]
name = "fotw"
socket_path = "auto"
wire_format = "json"  # JSON for debuggability; switch to "messagepack" for production throughput
api_port = 0

[event_store]
json_log_dir = "auto"
sqlite_path = "auto"
retention_days = 7

# =============================================================================
# Audio Sources (fan-in — add more by duplicating with different env)
# =============================================================================

[[sources]]
name = "mic-local"
path = "uv"
args = ["run", "--with", "emergent-client,sounddevice,numpy,onnxruntime", "python", "audio_capture.py"]
env = { AUDIO_DEVICE = "default", SPEAKER_LABEL = "me" }
publishes = ["audio.chunk"]

# Uncomment to capture speaker output (what remote participants say):
# [[sources]]
# name = "speakers"
# path = "uv"
# args = ["run", "--with", "emergent-client,sounddevice,numpy,onnxruntime", "python", "audio_capture.py"]
# env = { AUDIO_DEVICE = "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__HDMI1__sink.monitor", SPEAKER_LABEL = "remote" }
# publishes = ["audio.chunk"]

# Uncomment to capture from a remote/secondary microphone:
# [[sources]]
# name = "mic-remote"
# path = "uv"
# args = ["run", "--with", "emergent-client,sounddevice,numpy,onnxruntime", "python", "audio_capture.py"]
# env = { AUDIO_DEVICE = "alsa_input.usb-remote-mic", SPEAKER_LABEL = "guest" }
# publishes = ["audio.chunk"]

# =============================================================================
# Transcription Handler
# =============================================================================

[[handlers]]
name = "transcribe"
path = "~/.local/share/emergent/primitives/bin/exec-handler"
args = [
    "-s", "audio.chunk",
    "--publish-as", "transcript.segment",
    "--timeout", "30000",
    "--",
    "sh", "./transcribe.sh"
]
subscribes = ["audio.chunk"]
publishes = ["transcript.segment"]

# =============================================================================
# Sinks (fan-out)
# =============================================================================

[[sinks]]
name = "console"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = ["-s", "transcript.segment", "--", "sh", "./format_transcript.sh"]
subscribes = ["transcript.segment"]

[[sinks]]
name = "transcript"
path = "~/.local/share/emergent/primitives/bin/exec-sink"
args = [
    "-s", "transcript.segment",
    "--",
    "sh", "-c",
    "sh ./format_transcript.sh >> \"${FOTW_TRANSCRIPT_FILE:-./transcript-$(date +%Y-%m-%d).txt}\""
]
subscribes = ["transcript.segment"]
```

---

## Dependencies & Setup

### System packages (Arch Linux)

```bash
sudo pacman -S portaudio cuda cudnn jq
```

- `portaudio`: required by `sounddevice` for audio capture
- `cuda`, `cudnn`: required by whisper.cpp CUDA build
- `jq`: used by `transcribe.sh` and `format_transcript.sh` for JSON processing

PipeWire should already be running on a standard Arch desktop install.

### whisper.cpp (build with CUDA)

```bash
git clone https://github.com/ggerganov/whisper.cpp.git
cd whisper.cpp
cmake -B build -DGGML_CUDA=ON
cmake --build build --config Release -j$(nproc)

# Install the binary
sudo cp build/bin/whisper-cli /usr/local/bin/

# Download the large-v3 model
mkdir -p ~/.local/share/whisper.cpp/models
./models/download-ggml-model.sh large-v3
cp models/ggml-large-v3.bin ~/.local/share/whisper.cpp/models/
```

Alternatively, if whisper.cpp is packaged in AUR:
```bash
yay -S whisper.cpp-cuda
```

### Python dependencies (managed by uv, no global install)

The `uv run --with` pattern in the TOML config handles audio capture dependencies at runtime:

- **audio_capture.py**: `emergent-client`, `sounddevice`, `numpy`, `onnxruntime` (for Silero VAD ONNX model)

No virtualenv setup required — `uv` resolves and caches dependencies on first run.

### Emergent marketplace primitives

```bash
emergent marketplace install exec-handler exec-sink
```

### Verify whisper.cpp works with CUDA

```bash
# Quick smoke test — should print transcription and show CUDA in logs
echo "test" | whisper-cli -m ~/.local/share/whisper.cpp/models/ggml-large-v3.bin -f - 2>&1 | head -5
```

---

## Audio Device Discovery

List available PipeWire/PulseAudio sources:

```bash
# All sources (inputs)
pactl list sources short

# Monitor sources (capture speaker output)
pactl list sources short | grep monitor
```

Use the source name (e.g., `alsa_input.usb-Blue_Yeti-00.analog-stereo`) as the `AUDIO_DEVICE` env var. Use `default` for the system default input.

---

## Files to Create

```
~/code/active/emergent-fotw/
  emergent.toml            # Pipeline configuration
  audio_capture.py         # SDK source: mic capture + VAD + emit chunk file paths
  transcribe.sh            # Shell script: pipe WAV through whisper.cpp, emit JSON
  format_transcript.sh     # Shell script: format transcript.segment into readable lines
```

Four files total. One Python script, two shell scripts, one TOML config.

---

## Error Handling

| Failure | Behavior |
|---------|----------|
| whisper.cpp not installed or model missing | `transcribe.sh` exits non-zero. exec-handler does not publish `transcript.segment`. Audio file preserved. Error visible in engine output. |
| Audio file missing (race/stale path) | `transcribe.sh` detects missing file, exits non-zero with clear error message. |
| Audio device unavailable | `audio_capture.py` exits with error. Engine reports `system.error.mic-local`. Other sources continue. |
| GPU out of memory | whisper.cpp fails, script exits non-zero. Chunk skipped. Next chunk retries (GPU may have recovered). |
| Whisper inference timeout | exec-handler `--timeout 30000` kills the script. Chunk file may leak. |
| Silence (no speech) | VAD holds — no `audio.chunk` emitted. No whisper.cpp invocations. No temp files written. |
| One source crashes | Process isolation — other audio sources and transcription continue unaffected. |
| Temp file cleanup | Normal operation: `transcribe.sh` deletes each file after transcription. Orphaned files accumulate in `$XDG_RUNTIME_DIR/emergent-fotw/` (tmpfs, cleared on reboot). For long-running sessions, periodically clean files older than 10 minutes. |

---

## Future Extensions

These require only TOML changes, no modifications to existing components:

| Extension | How |
|-----------|-----|
| **Persistent model (throughput upgrade)** | Replace exec-handler with whisper.cpp's built-in `whisper-server` + `curl` — model loads once, one TOML change |
| **Summarization** | Add a handler subscribing to `transcript.segment`, pipe through `claude -p "Summarize"`, publish `meeting.summary` |
| **Action item extraction** | Same pattern — exec-handler with an LLM prompt targeting action items |
| **Real-time dashboard** | Add `sse-sink` subscribing to `transcript.segment`, serve an HTML page |
| **Slack posting** | Add exec-sink that curls Slack webhook with transcript segments |
| **Speaker diarization** | Enhance `audio_capture.py` with speaker embedding comparison, or add a diarization handler between capture and transcription |
| **Per-segment timestamps** | Remove `--no-timestamps` from whisper.cpp flags, parse timestamped output in `transcribe.sh` |

---

## Testing Plan

### Automated (repeatable without a microphone)

1. **whisper.cpp smoke test**: Run `whisper-cli -m <model> -f -` with a known test WAV piped to stdin. Verify text output.
2. **transcribe.sh standalone**: Create a test WAV, write a mock `audio.chunk` JSON with its path, pipe through the script, verify JSON output with speaker and timestamp.
3. **format_transcript.sh standalone**: Pipe a mock `transcript.segment` JSON, verify formatted text output.

### Integration (requires audio hardware)

4. **Audio capture standalone**: Run `audio_capture.py` with a known audio device, speak a test phrase, verify `audio.chunk` events emitted and temp WAV files created.
5. **Single-source pipeline**: Run full pipeline with just `mic-local`, verify console + file output.
6. **Multi-source pipeline**: Add speaker output capture, verify speaker attribution in transcript.
7. **Shutdown**: Ctrl+C, verify temp files are cleaned up, no zombie processes.
8. **Long-running**: Run for 30+ minutes, verify no memory leaks, transcript file grows correctly, temp directory stays clean.
9. **Throughput**: With multiple sources active, verify the message queue drains and transcript eventually catches up.
