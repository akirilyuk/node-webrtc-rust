#!/usr/bin/env bash
# Regenerates fixtures/quick-brown-fox-16k-mono.pcm with local Sherpa vits-piper-en_US-amy-low.
#
# Model lookup, first match wins: $SHERPA_TTS_MODEL_PATH, $NWR_CI_SHERPA_MODELS/vits-piper-en_US-amy-low,
# examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low (what `ensure-sherpa-models.sh` downloads).
# Piper noise differs per render: the fixture is checked in and regenerated only on purpose.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

MODEL="${SHERPA_TTS_MODEL_PATH:-}"
if [ -z "$MODEL" ] && [ -n "${NWR_CI_SHERPA_MODELS:-}" ]; then
  MODEL="$NWR_CI_SHERPA_MODELS/vits-piper-en_US-amy-low"
fi
if [ -z "$MODEL" ]; then
  MODEL="examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low"
fi
if [ ! -d "$MODEL" ]; then
  echo "Sherpa model not found: $MODEL (run scripts/ci/ensure-sherpa-models.sh or set SHERPA_TTS_MODEL_PATH)" >&2
  exit 1
fi

export SHERPA_TTS_MODEL_PATH="$MODEL"
export FIXTURE_OUT="$ROOT/crates/vendor-live-support/fixtures/quick-brown-fox-16k-mono.pcm"
cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test make_stt_fixture_test -- --ignored --nocapture --test-threads=1
