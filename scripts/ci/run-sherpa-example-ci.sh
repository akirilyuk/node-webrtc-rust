#!/usr/bin/env bash
# Sherpa local example CI — typecheck and/or semantic barge-in E2E (downloads models on demand).
#
# Usage (from repo root):
#   bash scripts/ci/run-sherpa-example-ci.sh typecheck
#   bash scripts/ci/run-sherpa-example-ci.sh e2e      # requires linux-gnu/host .node in packages/bindings/
#   bash scripts/ci/run-sherpa-example-ci.sh all
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORKSPACE="@node-webrtc-rust/example-voice-agent-local-sherpa"
EXAMPLE_ROOT="$ROOT/examples/voice-agent-local-sherpa"
STT_BUNDLE="sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06"
TTS_BUNDLE="vits-piper-en_US-amy-low"

ensure_ts_dist() {
  if [[ ! -f packages/sdk/dist/cjs/index.js ]] \
    || [[ ! -f packages/signaling/dist/cjs/index.js ]] \
    || [[ ! -f packages/helpers/dist/cjs/index.js ]]; then
    echo "==> build TypeScript workspace (dist missing for Sherpa example typecheck)"
    bash "$ROOT/scripts/ci/build-ts-workspace.sh"
  fi
}

run_typecheck() {
  ensure_ts_dist
  echo "==> typecheck ($WORKSPACE)"
  npm run typecheck --workspace="$WORKSPACE"
}

ensure_sherpa_models() {
  echo "==> Sherpa STT/TTS model weights ($WORKSPACE)"
  npm run download-stt:en --workspace="$WORKSPACE"
  npm run download-tts:en --workspace="$WORKSPACE"

  export SHERPA_STT_MODEL_PATH="$EXAMPLE_ROOT/.models/$STT_BUNDLE"
  export SHERPA_TTS_MODEL_PATH="$EXAMPLE_ROOT/.models/$TTS_BUNDLE"
  export SHERPA_STT_LANGUAGE="${SHERPA_STT_LANGUAGE:-en}"

  if [[ ! -d "$SHERPA_STT_MODEL_PATH" ]] || [[ ! -d "$SHERPA_TTS_MODEL_PATH" ]]; then
    echo "Sherpa model directories missing after download." >&2
    echo "  STT: $SHERPA_STT_MODEL_PATH" >&2
    echo "  TTS: $SHERPA_TTS_MODEL_PATH" >&2
    exit 1
  fi
}

run_e2e() {
  shopt -s nullglob
  local nodes=(packages/bindings/*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "No native .node in packages/bindings — required for Sherpa E2E." >&2
    exit 1
  fi

  ensure_ts_dist
  ensure_sherpa_models

  echo "==> semantic barge-in E2E ($WORKSPACE)"
  npm run start:roundtrip-barge-in --workspace="$WORKSPACE"
}

mode="${1:-all}"
case "$mode" in
  typecheck) run_typecheck ;;
  e2e) run_e2e ;;
  all)
    run_typecheck
    run_e2e
    ;;
  *)
    echo "Usage: $0 {typecheck|e2e|all}" >&2
    exit 2
    ;;
esac

echo "==> Sherpa example CI ($mode) OK"
