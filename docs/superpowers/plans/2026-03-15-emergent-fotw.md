# emergent-fotw Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a multi-stream meeting transcription pipeline using whisper.cpp and Emergent's pub-sub engine.

**Architecture:** SDK-based Python audio sources capture mic/speaker audio with VAD chunking, emit file paths via pub-sub. A stateless exec-handler pipes each WAV through `whisper-cli -f -` and publishes transcript text. Exec-sinks format and output to console and file.

**Tech Stack:** Emergent engine, whisper.cpp (CUDA), Python (sounddevice, Silero VAD ONNX, emergent-client SDK), jq, shell scripts.

**Spec:** `docs/superpowers/specs/2026-03-15-emergent-fotw-design.md`

**Project directory:** `~/code/active/emergent-fotw`

---

## File Structure

```
~/code/active/emergent-fotw/
  emergent.toml            # Pipeline TOML config
  audio_capture.py         # SDK source: mic + VAD → temp WAV files → audio.chunk events
  vad.py                   # VadStateMachine — pure logic, no external deps beyond numpy
  transcribe.sh            # exec-handler script: WAV → whisper.cpp → JSON
  format_transcript.sh     # exec-sink script: transcript.segment JSON → readable text
  tests/
    test_format.sh         # Automated test for format_transcript.sh
    test_transcribe.sh     # Automated test for transcribe.sh (requires whisper-cli)
    test_vad.py            # Unit tests for VAD state machine (no hardware)
    fixtures/
      hello.wav            # Short test audio file for whisper.cpp smoke tests
```

Note: `VadStateMachine` lives in its own `vad.py` module so that VAD unit tests can import it without pulling in `sounddevice`, `onnxruntime`, or `emergent-client` dependencies. `audio_capture.py` imports from `vad.py`.

---

## Chunk 1: Shell Scripts and TOML Config

### Task 1: Create project directory and git repo

**Files:**
- Create: `~/code/active/emergent-fotw/`

- [ ] **Step 1: Initialize project**

```bash
cd ~/code/active/emergent-fotw
git init
```

- [ ] **Step 2: Create .gitignore**

```
# Transcripts
transcript-*.txt

# Python
__pycache__/
*.pyc

# Temp audio files
*.wav
# But keep test fixtures
!tests/fixtures/*.wav

# Emergent runtime
.emergent/
```

- [ ] **Step 3: Commit**

```bash
git add .gitignore
git commit -S -m "chore: initialize emergent-fotw project"
```

---

### Task 2: Write format_transcript.sh with test

**Files:**
- Create: `~/code/active/emergent-fotw/format_transcript.sh`
- Create: `~/code/active/emergent-fotw/tests/test_format.sh`

- [ ] **Step 1: Create tests directory and write the test script**

```bash
mkdir -p ~/code/active/emergent-fotw/tests
```

Create `tests/test_format.sh`:

```bash
#!/bin/sh
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PASS=0
FAIL=0

# Test 1: Basic formatting
expected='[2026-03-15T20:30:00Z] me: Hello everyone.'
actual=$(echo '{"speaker":"me","text":"Hello everyone.","timestamp_utc":"2026-03-15T20:30:00Z"}' | sh "$SCRIPT_DIR/format_transcript.sh")

if [ "$actual" = "$expected" ]; then
  echo "PASS: basic formatting"
  PASS=$((PASS + 1))
else
  echo "FAIL: basic formatting"
  echo "  expected: $expected"
  echo "  actual:   $actual"
  FAIL=$((FAIL + 1))
fi

# Test 2: Text with special characters
expected='[2026-03-15T20:31:00Z] remote: She said "hello" & waved.'
actual=$(echo '{"speaker":"remote","text":"She said \"hello\" & waved.","timestamp_utc":"2026-03-15T20:31:00Z"}' | sh "$SCRIPT_DIR/format_transcript.sh")

if [ "$actual" = "$expected" ]; then
  echo "PASS: special characters"
  PASS=$((PASS + 1))
else
  echo "FAIL: special characters"
  echo "  expected: $expected"
  echo "  actual:   $actual"
  FAIL=$((FAIL + 1))
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/code/active/emergent-fotw
sh tests/test_format.sh
```

Expected: FAIL — `format_transcript.sh` does not exist yet.

- [ ] **Step 3: Write format_transcript.sh**

Create `format_transcript.sh`:

```bash
#!/bin/sh
# Format a transcript.segment JSON payload into a readable transcript line.
# Reads JSON from stdin, outputs formatted text to stdout.
jq -r '"[\(.timestamp_utc)] \(.speaker): \(.text)"'
```

- [ ] **Step 4: Make scripts executable and run test to verify it passes**

```bash
chmod +x format_transcript.sh tests/test_format.sh
sh tests/test_format.sh
```

Expected: `2 passed, 0 failed`

- [ ] **Step 5: Commit**

```bash
git add format_transcript.sh tests/test_format.sh
git commit -S -m "feat: add format_transcript.sh with tests"
```

---

### Task 3: Write transcribe.sh with test

**Files:**
- Create: `~/code/active/emergent-fotw/transcribe.sh`
- Create: `~/code/active/emergent-fotw/tests/test_transcribe.sh`
- Create: `~/code/active/emergent-fotw/tests/fixtures/` (directory)

This task requires `whisper-cli` and the `ggml-large-v3.bin` model to be installed. If not yet installed, follow the setup in the spec's "Dependencies & Setup" section first.

- [ ] **Step 1: Generate a test WAV fixture**

Create a short silent WAV file for testing (whisper.cpp will return empty/whitespace text for silence, which is fine for a structural test):

```bash
mkdir -p ~/code/active/emergent-fotw/tests/fixtures
# Generate 1 second of silence at 16kHz mono 16-bit PCM WAV
ffmpeg -f lavfi -i anullsrc=r=16000:cl=mono -t 1 -c:a pcm_s16le \
  ~/code/active/emergent-fotw/tests/fixtures/hello.wav -y 2>/dev/null
```

- [ ] **Step 2: Write the test script**

Create `tests/test_transcribe.sh`. The test copies the fixture to a temp file before passing it to `transcribe.sh` (which deletes the input file), preserving the original fixture for re-runs.

```bash
#!/bin/sh
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE_DIR="$SCRIPT_DIR/tests/fixtures"
PASS=0
FAIL=0

# Verify whisper-cli is available
if ! command -v whisper-cli >/dev/null 2>&1; then
  echo "SKIP: whisper-cli not found in PATH"
  exit 0
fi

# Verify model exists
WHISPER_MODEL="${WHISPER_MODEL:-${HOME}/.local/share/whisper.cpp/models/ggml-large-v3.bin}"
if [ ! -f "$WHISPER_MODEL" ]; then
  echo "SKIP: whisper model not found at $WHISPER_MODEL"
  exit 0
fi

# Test 1: Valid audio file produces JSON with required fields
# Copy fixture to temp file (transcribe.sh deletes input files)
tmp_wav=$(mktemp /tmp/fotw-test-XXXXXX.wav)
cp "$FIXTURE_DIR/hello.wav" "$tmp_wav"

input=$(jq -nc --arg p "$tmp_wav" --arg s "test-speaker" '{audio_path: $p, speaker: $s}')
output=$(echo "$input" | sh "$SCRIPT_DIR/transcribe.sh" 2>/dev/null)

# Verify output is valid JSON with expected fields
speaker=$(echo "$output" | jq -r .speaker)
timestamp=$(echo "$output" | jq -r .timestamp_utc)
has_text=$(echo "$output" | jq 'has("text")')

if [ "$speaker" = "test-speaker" ] && [ "$has_text" = "true" ] && [ "$timestamp" != "null" ]; then
  echo "PASS: valid audio produces JSON with speaker, text, timestamp_utc"
  PASS=$((PASS + 1))
else
  echo "FAIL: valid audio output"
  echo "  output: $output"
  FAIL=$((FAIL + 1))
fi

# Test 2: Missing audio file exits non-zero
input=$(jq -nc '{audio_path: "/nonexistent/file.wav", speaker: "nobody"}')
if echo "$input" | sh "$SCRIPT_DIR/transcribe.sh" 2>/dev/null; then
  echo "FAIL: missing file should exit non-zero"
  FAIL=$((FAIL + 1))
else
  echo "PASS: missing file exits non-zero"
  PASS=$((PASS + 1))
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cd ~/code/active/emergent-fotw
sh tests/test_transcribe.sh
```

Expected: FAIL — `transcribe.sh` does not exist yet.

- [ ] **Step 4: Write transcribe.sh**

Create `transcribe.sh`:

```bash
#!/bin/sh
set -e

WHISPER_MODEL="${WHISPER_MODEL:-${HOME}/.local/share/whisper.cpp/models/ggml-large-v3.bin}"
WHISPER_BIN="${WHISPER_BIN:-whisper-cli}"

# Read payload from stdin (exec-handler sends payload JSON only)
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

- [ ] **Step 5: Make scripts executable and run test**

```bash
chmod +x transcribe.sh tests/test_transcribe.sh
sh tests/test_transcribe.sh
```

Expected: `2 passed, 0 failed` (or `SKIP` if whisper-cli not installed yet)

- [ ] **Step 6: Commit**

```bash
git add transcribe.sh tests/test_transcribe.sh tests/fixtures/hello.wav
git commit -S -m "feat: add transcribe.sh with tests"
```

---

### Task 4: Write emergent.toml

**Files:**
- Create: `~/code/active/emergent-fotw/emergent.toml`

Note: the TOML config is validated semantically by the integration test in Task 9 rather than by a unit test — there is no unit test framework for TOML pipeline configs.

- [ ] **Step 1: Write the TOML config**

Create `emergent.toml`:

```toml
[engine]
name = "fotw"
socket_path = "auto"
wire_format = "json"  # JSON for debuggability; switch to "messagepack" for production
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

- [ ] **Step 2: Validate the TOML parses correctly**

```bash
cd ~/code/active/emergent-fotw
python3 -c "import tomllib; tomllib.load(open('emergent.toml', 'rb')); print('TOML valid')"
```

Expected: `TOML valid`

- [ ] **Step 3: Commit**

```bash
git add emergent.toml
git commit -S -m "feat: add emergent.toml pipeline configuration"
```

---

## Chunk 2: Audio Capture Source

### Task 5: Write VAD state machine (pure logic, testable without hardware)

**Files:**
- Create: `~/code/active/emergent-fotw/vad.py`
- Create: `~/code/active/emergent-fotw/tests/test_vad.py`

The VAD state machine is the core logic. It lives in its own `vad.py` module with only a `numpy` dependency so unit tests can run without `sounddevice`, `onnxruntime`, or `emergent-client`.

- [ ] **Step 1: Write the failing test**

Create `tests/test_vad.py`:

```python
"""Tests for the VAD state machine — no audio hardware or ONNX model needed.

The VadStateMachine is tested with synthetic speech probabilities
instead of real audio, isolating the chunking logic from audio capture
and model inference.
"""

import numpy as np
import pytest

from vad import VadStateMachine


SAMPLE_RATE = 16000
FRAME_MS = 30
FRAME_SAMPLES = int(SAMPLE_RATE * FRAME_MS / 1000)  # 480 samples


def make_frame(value: float = 0.0) -> np.ndarray:
    """Create a 30ms audio frame filled with a constant value."""
    return np.full(FRAME_SAMPLES, value, dtype=np.int16)


class TestVadStateMachine:
    def test_starts_idle(self):
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=1500,
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )
        assert vad.state == "idle"

    def test_no_chunks_emitted_during_silence(self):
        """When all frames are below threshold, no chunks should be produced."""
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=1500,
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )
        chunks = []
        for _ in range(100):
            chunk = vad.process_frame(make_frame(), speech_prob=0.1)
            if chunk is not None:
                chunks.append(chunk)
        assert len(chunks) == 0
        assert vad.state == "idle"

    def test_speech_then_silence_emits_chunk(self):
        """Speech followed by enough silence should emit exactly one chunk."""
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=300,  # Short for testing: 10 frames * 30ms
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )
        chunks = []

        # 20 frames of speech (600ms)
        for _ in range(20):
            chunk = vad.process_frame(make_frame(1000), speech_prob=0.9)
            if chunk is not None:
                chunks.append(chunk)
        assert vad.state == "recording"

        # 15 frames of silence (450ms > 300ms threshold)
        for _ in range(15):
            chunk = vad.process_frame(make_frame(0), speech_prob=0.1)
            if chunk is not None:
                chunks.append(chunk)

        assert len(chunks) == 1
        assert vad.state == "idle"

    def test_chunk_contains_speech_audio(self):
        """The emitted chunk should contain the speech frames."""
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=300,
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )

        # 10 frames of speech
        for _ in range(10):
            vad.process_frame(make_frame(1000), speech_prob=0.9)

        # Enough silence to trigger chunk
        chunk = None
        for _ in range(15):
            result = vad.process_frame(make_frame(0), speech_prob=0.1)
            if result is not None:
                chunk = result

        assert chunk is not None
        # Chunk should have at least the speech frames (may include pre-roll + trailing silence)
        min_samples = 10 * FRAME_SAMPLES
        assert len(chunk) >= min_samples

    def test_preroll_captures_speech_onset(self):
        """The ring buffer pre-roll should capture frames before VAD triggers."""
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=300,
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )

        # Feed 20 frames of "silence" with a distinctive value (pre-roll buffer)
        for _ in range(20):
            vad.process_frame(make_frame(42), speech_prob=0.1)

        # Now speech starts
        for _ in range(10):
            vad.process_frame(make_frame(1000), speech_prob=0.9)

        # Silence to flush
        chunk = None
        for _ in range(15):
            result = vad.process_frame(make_frame(0), speech_prob=0.1)
            if result is not None:
                chunk = result

        assert chunk is not None
        # Pre-roll is 500ms = ~16 frames at 30ms each. Check chunk starts with pre-roll values.
        # The chunk should start with pre-roll data (value=42), not speech data (value=1000)
        assert chunk[0] == 42

    def test_flush_emits_in_progress_recording(self):
        """flush() should emit the current recording if in recording state."""
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=1500,
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )

        # Start recording
        for _ in range(10):
            vad.process_frame(make_frame(1000), speech_prob=0.9)
        assert vad.state == "recording"

        # Flush without waiting for silence
        chunk = vad.flush()
        assert chunk is not None
        assert len(chunk) > 0
        assert vad.state == "idle"

    def test_flush_returns_none_when_idle(self):
        """flush() should return None if not currently recording."""
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=1500,
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )
        assert vad.flush() is None

    def test_multiple_utterances(self):
        """Two speech segments separated by silence should emit two chunks."""
        vad = VadStateMachine(
            vad_threshold=0.5,
            silence_duration_ms=300,
            sample_rate=SAMPLE_RATE,
            frame_ms=FRAME_MS,
        )
        chunks = []

        # Utterance 1
        for _ in range(10):
            r = vad.process_frame(make_frame(1000), speech_prob=0.9)
            if r is not None:
                chunks.append(r)
        for _ in range(15):
            r = vad.process_frame(make_frame(0), speech_prob=0.1)
            if r is not None:
                chunks.append(r)

        # Utterance 2
        for _ in range(10):
            r = vad.process_frame(make_frame(2000), speech_prob=0.8)
            if r is not None:
                chunks.append(r)
        for _ in range(15):
            r = vad.process_frame(make_frame(0), speech_prob=0.1)
            if r is not None:
                chunks.append(r)

        assert len(chunks) == 2
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/code/active/emergent-fotw
uv run --with numpy,pytest pytest tests/test_vad.py -v
```

Expected: `ModuleNotFoundError` or `ImportError` — `vad` module doesn't exist yet.

- [ ] **Step 3: Write VadStateMachine in vad.py**

Create `vad.py`:

```python
"""VAD (Voice Activity Detection) state machine for audio chunking.

Pure logic — depends only on numpy. No audio hardware, no ONNX model,
no Emergent SDK. This module is imported by audio_capture.py and tested
independently in tests/test_vad.py.
"""

from __future__ import annotations

import collections
from typing import Optional

import numpy as np


class VadStateMachine:
    """Detects speech boundaries and emits audio chunks.

    Implements a two-state machine (idle/recording) with a ring buffer
    pre-roll to capture speech onset before VAD triggers.

    Args:
        vad_threshold: Silero VAD probability threshold for speech detection.
        silence_duration_ms: Milliseconds of silence to trigger chunk boundary.
        sample_rate: Audio sample rate in Hz.
        frame_ms: Frame duration in milliseconds.
    """

    def __init__(
        self,
        vad_threshold: float = 0.5,
        silence_duration_ms: int = 1500,
        sample_rate: int = 16000,
        frame_ms: int = 30,
    ) -> None:
        self.vad_threshold = vad_threshold
        self.silence_duration_ms = silence_duration_ms
        self.sample_rate = sample_rate
        self.frame_ms = frame_ms
        self.frame_samples = int(sample_rate * frame_ms / 1000)

        # Pre-roll ring buffer: 500ms of frames
        preroll_frames = int(500 / frame_ms)
        self._ring_buffer: collections.deque[np.ndarray] = collections.deque(
            maxlen=preroll_frames
        )

        # Recording state
        self.state: str = "idle"
        self._accumulator: list[np.ndarray] = []
        self._silence_frames: int = 0
        self._silence_frame_threshold = int(silence_duration_ms / frame_ms)

    def process_frame(
        self, frame: np.ndarray, speech_prob: float
    ) -> Optional[np.ndarray]:
        """Process one audio frame and return a chunk if a speech segment ended.

        Args:
            frame: Audio samples for this frame (int16 numpy array).
            speech_prob: Speech probability from VAD model (0.0 to 1.0).

        Returns:
            Concatenated audio chunk (int16 numpy array) if a speech segment
            ended, otherwise None.
        """
        is_speech = speech_prob >= self.vad_threshold

        if self.state == "idle":
            if is_speech:
                # Transition to recording — prepend pre-roll buffer
                self.state = "recording"
                self._accumulator = list(self._ring_buffer)
                self._accumulator.append(frame)
                self._silence_frames = 0
            else:
                # Stay idle — feed ring buffer
                self._ring_buffer.append(frame)
            return None

        # state == "recording"
        self._accumulator.append(frame)

        if is_speech:
            self._silence_frames = 0
            return None

        # Silence frame during recording
        self._silence_frames += 1
        if self._silence_frames >= self._silence_frame_threshold:
            # Enough silence — emit chunk and reset
            chunk = np.concatenate(self._accumulator)
            self._accumulator = []
            self._silence_frames = 0
            self.state = "idle"
            return chunk

        return None

    def flush(self) -> Optional[np.ndarray]:
        """Flush any in-progress recording (e.g., on shutdown).

        Returns:
            Audio chunk if recording was in progress, otherwise None.
        """
        if self.state != "recording" or not self._accumulator:
            return None

        chunk = np.concatenate(self._accumulator)
        self._accumulator = []
        self._silence_frames = 0
        self.state = "idle"
        return chunk
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd ~/code/active/emergent-fotw
uv run --with numpy,pytest pytest tests/test_vad.py -v
```

Expected: All 8 tests pass.

- [ ] **Step 5: Commit**

```bash
git add vad.py tests/test_vad.py
git commit -S -m "feat: add VadStateMachine with tests"
```

---

### Task 6: Wire audio capture with sounddevice, Silero VAD, and Emergent SDK

**Files:**
- Create: `~/code/active/emergent-fotw/audio_capture.py`

This task creates the main entry point that wires together: sounddevice audio stream → Silero VAD ONNX inference → VadStateMachine → WAV file writing → Emergent SDK publishing. This code requires audio hardware to test and is covered by the integration test in Task 9.

- [ ] **Step 1: Write audio_capture.py**

Create `audio_capture.py`:

```python
"""Audio capture source for emergent-fotw.

Captures audio from a PipeWire/PulseAudio device, detects speech boundaries
using Silero VAD (ONNX), writes chunks to temp WAV files, and publishes
audio.chunk events via the Emergent Python SDK.

Run via: uv run --with emergent-client,sounddevice,numpy,onnxruntime python audio_capture.py
"""

from __future__ import annotations

import asyncio
import os
import sys
import time
import wave
from typing import Optional

import numpy as np
import onnxruntime
import sounddevice as sd

from vad import VadStateMachine


def load_silero_vad(model_path: Optional[str] = None) -> onnxruntime.InferenceSession:
    """Load the Silero VAD ONNX model.

    Downloads the model on first run if not found at the expected path.

    Args:
        model_path: Explicit path to silero_vad.onnx. If None, checks
            $XDG_DATA_HOME/silero-vad/silero_vad.onnx.

    Returns:
        ONNX InferenceSession ready for inference.
    """
    if model_path is None:
        xdg_data = os.environ.get(
            "XDG_DATA_HOME", os.path.expanduser("~/.local/share")
        )
        model_path = os.path.join(xdg_data, "silero-vad", "silero_vad.onnx")

    if not os.path.exists(model_path):
        import urllib.request

        os.makedirs(os.path.dirname(model_path), exist_ok=True)
        url = "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx"
        print(f"Downloading Silero VAD model to {model_path}...", flush=True)
        urllib.request.urlretrieve(url, model_path)
        print("Done.", flush=True)

    return onnxruntime.InferenceSession(
        model_path, providers=["CPUExecutionProvider"]
    )


def silero_vad_predict(
    session: onnxruntime.InferenceSession,
    audio_frame: np.ndarray,
    state: np.ndarray,
    sample_rate: int = 16000,
) -> tuple[float, np.ndarray]:
    """Run Silero VAD inference on a single audio frame.

    Args:
        session: ONNX InferenceSession with Silero VAD model loaded.
        audio_frame: Audio samples (int16 numpy array, 30ms at 16kHz = 480 samples).
        state: Hidden state from previous call (zeros on first call).
        sample_rate: Audio sample rate.

    Returns:
        Tuple of (speech_probability, updated_state).
    """
    # Silero VAD expects float32 in [-1, 1]
    audio_float = audio_frame.astype(np.float32) / 32768.0
    audio_float = audio_float[np.newaxis, :]  # Add batch dimension

    sr = np.array([sample_rate], dtype=np.int64)

    outputs = session.run(
        None,
        {"input": audio_float, "state": state, "sr": sr},
    )
    prob = outputs[0].item()
    new_state = outputs[1]

    return prob, new_state


def write_wav(path: str, audio: np.ndarray, sample_rate: int = 16000) -> None:
    """Write int16 PCM audio to a WAV file.

    Args:
        path: Output file path.
        audio: Audio samples (int16 numpy array).
        sample_rate: Sample rate in Hz.
    """
    with wave.open(path, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit
        wf.setframerate(sample_rate)
        wf.writeframes(audio.tobytes())


async def run(source, shutdown_event: asyncio.Event) -> None:
    """Main audio capture loop.

    Captures audio from a PipeWire/PulseAudio device, runs VAD, writes
    speech chunks to temp WAV files, and publishes audio.chunk events.
    """
    from emergent import create_message

    # Configuration from environment
    device_name = os.environ.get("AUDIO_DEVICE", "default")
    speaker_label = os.environ.get("SPEAKER_LABEL", "unknown")
    vad_threshold = float(os.environ.get("VAD_THRESHOLD", "0.5"))
    silence_ms = int(os.environ.get("SILENCE_DURATION_MS", "1500"))
    sample_rate = int(os.environ.get("SAMPLE_RATE", "16000"))
    frame_ms = 30
    frame_samples = int(sample_rate * frame_ms / 1000)

    # Temp directory for WAV chunks
    runtime_dir = os.environ.get("XDG_RUNTIME_DIR", "/tmp")
    chunk_dir = os.path.join(runtime_dir, "emergent-fotw")
    os.makedirs(chunk_dir, exist_ok=True)

    # Load Silero VAD
    vad_session = load_silero_vad()
    vad_state = np.zeros((2, 1, 128), dtype=np.float32)  # Silero VAD hidden state

    # VAD state machine
    vad = VadStateMachine(
        vad_threshold=vad_threshold,
        silence_duration_ms=silence_ms,
        sample_rate=sample_rate,
        frame_ms=frame_ms,
    )

    # Audio callback writes frames to a queue
    audio_queue: asyncio.Queue[np.ndarray] = asyncio.Queue()
    loop = asyncio.get_running_loop()

    def audio_callback(indata, frames, time_info, status):
        if status:
            print(f"Audio status: {status}", file=sys.stderr, flush=True)
        # Validate frame size — sounddevice may deliver short frames at stream edges
        if indata.shape[0] == frame_samples:
            loop.call_soon_threadsafe(audio_queue.put_nowait, indata[:, 0].copy())

    # Resolve device
    device = None if device_name == "default" else device_name

    print(
        f"Starting audio capture: device={device_name}, speaker={speaker_label}",
        file=sys.stderr,
        flush=True,
    )

    primitive_name = os.environ.get("EMERGENT_NAME", "mic")

    with sd.InputStream(
        device=device,
        samplerate=sample_rate,
        channels=1,
        dtype="int16",
        blocksize=frame_samples,
        callback=audio_callback,
    ):
        while not shutdown_event.is_set():
            try:
                frame = await asyncio.wait_for(audio_queue.get(), timeout=0.1)
            except asyncio.TimeoutError:
                continue

            # Run VAD inference
            prob, vad_state = silero_vad_predict(
                vad_session, frame, vad_state, sample_rate
            )

            # Process through state machine
            chunk = vad.process_frame(frame, prob)
            if chunk is not None:
                # Write chunk to temp WAV
                timestamp_ms = int(time.time() * 1000)
                duration_ms = int(len(chunk) / sample_rate * 1000)
                wav_path = os.path.join(
                    chunk_dir, f"chunk_{timestamp_ms}_{primitive_name}.wav"
                )
                write_wav(wav_path, chunk, sample_rate)

                # Publish audio.chunk event
                await source.publish(
                    create_message("audio.chunk").payload(
                        {
                            "audio_path": wav_path,
                            "speaker": speaker_label,
                            "duration_ms": duration_ms,
                            "sample_rate": sample_rate,
                        }
                    )
                )
                print(
                    f"Published chunk: {duration_ms}ms ({wav_path})",
                    file=sys.stderr,
                    flush=True,
                )

    # Flush any in-progress recording on shutdown
    final_chunk = vad.flush()
    if final_chunk is not None:
        timestamp_ms = int(time.time() * 1000)
        duration_ms = int(len(final_chunk) / sample_rate * 1000)
        wav_path = os.path.join(
            chunk_dir, f"chunk_{timestamp_ms}_{primitive_name}_final.wav"
        )
        write_wav(wav_path, final_chunk, sample_rate)
        await source.publish(
            create_message("audio.chunk").payload(
                {
                    "audio_path": wav_path,
                    "speaker": speaker_label,
                    "duration_ms": duration_ms,
                    "sample_rate": sample_rate,
                }
            )
        )


async def main() -> None:
    """Entry point — connect to Emergent and start capturing."""
    from emergent import run_source

    await run_source(None, run)


if __name__ == "__main__":
    asyncio.run(main())
```

- [ ] **Step 2: Verify the file has no syntax errors**

```bash
cd ~/code/active/emergent-fotw
uv run --with numpy,onnxruntime,sounddevice,emergent-client python -c "import audio_capture; print('Import OK')"
```

Expected: `Import OK`

- [ ] **Step 3: Re-run VAD tests to ensure nothing broke**

Since `VadStateMachine` lives in `vad.py` (no heavy deps), the tests still only need `numpy` and `pytest`:

```bash
uv run --with numpy,pytest pytest tests/test_vad.py -v
```

Expected: All 8 tests still pass.

- [ ] **Step 4: Commit**

```bash
git add audio_capture.py
git commit -S -m "feat: add audio capture with sounddevice, Silero VAD, and Emergent SDK"
```

---

## Chunk 3: Integration Testing

### Task 7: Verify prerequisites are installed

- [ ] **Step 1: Check whisper-cli is in PATH**

```bash
which whisper-cli
```

Expected: A path like `/usr/local/bin/whisper-cli`. If missing, follow the spec's "whisper.cpp (build with CUDA)" setup section.

- [ ] **Step 2: Check the model exists**

```bash
ls -lh ~/.local/share/whisper.cpp/models/ggml-large-v3.bin
```

Expected: File exists, ~3 GB.

- [ ] **Step 3: Check marketplace primitives are installed**

```bash
ls ~/.local/share/emergent/primitives/bin/exec-handler
ls ~/.local/share/emergent/primitives/bin/exec-sink
```

Expected: Both files exist.

- [ ] **Step 4: Check jq is available**

```bash
jq --version
```

Expected: Version output like `jq-1.7.1`.

- [ ] **Step 5: Check audio devices**

```bash
pactl list sources short
```

Expected: At least one source listed. Note the default input device name.

---

### Task 8: Run all automated tests

- [ ] **Step 1: Run format_transcript.sh tests**

```bash
cd ~/code/active/emergent-fotw
sh tests/test_format.sh
```

Expected: `2 passed, 0 failed`

- [ ] **Step 2: Run transcribe.sh tests**

```bash
sh tests/test_transcribe.sh
```

Expected: `2 passed, 0 failed` (test copies fixture to temp, original preserved)

- [ ] **Step 3: Run VAD state machine tests**

```bash
uv run --with numpy,pytest pytest tests/test_vad.py -v
```

Expected: All 8 tests pass.

---

### Task 9: Integration test — single source pipeline

- [ ] **Step 1: Start the pipeline**

```bash
cd ~/code/active/emergent-fotw
emergent --config ./emergent.toml
```

- [ ] **Step 2: Speak into the microphone**

Say a clear test phrase like "Hello, this is a test of the fly on the wall transcription system."

- [ ] **Step 3: Verify console output**

Expected: After a few seconds delay (model loading + inference), a formatted transcript line appears in the terminal:

```
[2026-03-15T...Z] me: Hello, this is a test of the fly on the wall transcription system.
```

- [ ] **Step 4: Verify file output**

```bash
cat transcript-$(date +%Y-%m-%d).txt
```

Expected: Same formatted line(s) appended to the file.

- [ ] **Step 5: Verify temp file cleanup**

```bash
ls $XDG_RUNTIME_DIR/emergent-fotw/
```

Expected: Directory is empty (temp WAV files deleted after transcription).

- [ ] **Step 6: Shutdown**

Press Ctrl+C. Verify the engine shuts down cleanly with no errors or zombie processes:

```bash
ps aux | grep -E "audio_capture|whisper-cli|exec-handler" | grep -v grep
```

Expected: No matching processes.

- [ ] **Step 7: Commit final state**

```bash
cd ~/code/active/emergent-fotw
git add audio_capture.py vad.py emergent.toml transcribe.sh format_transcript.sh tests/
git commit -S -m "feat: complete emergent-fotw pipeline — audio capture, transcription, output"
```
