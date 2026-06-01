#!/usr/bin/env bash
# Sherpa local example CI — typecheck, roundtrip Vitest evaluators, and/or Sherpa E2E.
#
# Usage (from repo root):
#   bash scripts/ci/run-sherpa-example-ci.sh typecheck
#   bash scripts/ci/run-sherpa-example-ci.sh vitest    # no models / .node
#   bash scripts/ci/run-sherpa-example-ci.sh e2e       # all start:roundtrip-* (models + .node)
#   bash scripts/ci/run-sherpa-example-ci.sh all       # typecheck + vitest + e2e
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CI_STEP="$ROOT/scripts/ci/ci-step.sh"
# Per roundtrip script — must exceed in-process SHERPA_ROUNDTRIP_WALL_MS (often 70–120s).
DEFAULT_SHERPA_ROUNDTRIP_TIMEOUT_SEC="${CI_SHERPA_ROUNDTRIP_TIMEOUT_SEC:-180}"
DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC="${CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC:-900}"

WORKSPACE="@node-webrtc-rust/example-voice-agent-local-sherpa"
EXAMPLE_ROOT="$ROOT/examples/voice-agent-local-sherpa"
STT_BUNDLE="sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06"
TTS_BUNDLE="vits-piper-en_US-amy-low"

# Sherpa roundtrip E2E entry points (see examples/voice-agent-local-sherpa/ROUNDTRIP.md).
SHERPA_ROUNDTRIP_E2E=(
  start:roundtrip-counting
  start:roundtrip-utterance-timing
  start:roundtrip-two-phrases
  start:roundtrip-barge-in
  start:roundtrip-counting-echo
  start:roundtrip-counting-barge-recovery
  start:roundtrip
)

ensure_ts_dist() {
  if [[ ! -f packages/sdk/dist/cjs/index.js ]] \
    || [[ ! -f packages/signaling/dist/cjs/index.js ]] \
    || [[ ! -f packages/helpers/dist/cjs/index.js ]]; then
    echo "==> build TypeScript workspace (dist missing for Sherpa example typecheck)"
    bash "$ROOT/scripts/ci/build-ts-workspace.sh"
  fi
}

ensure_native_node() {
  shopt -s nullglob
  local nodes=(packages/bindings/*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "No native .node in packages/bindings — required for Sherpa E2E." >&2
    echo "Run: npm run build:native" >&2
    exit 1
  fi
}

run_typecheck() {
  ensure_ts_dist
  echo "==> typecheck ($WORKSPACE)"
  npm run typecheck --workspace="$WORKSPACE"
}

run_vitest() {
  ensure_ts_dist
  echo "==> roundtrip Vitest evaluators ($WORKSPACE)"
  npm run test:roundtrip-counting --workspace="$WORKSPACE"
}

ensure_sherpa_models() {
  echo "==> Sherpa STT/TTS model weights ($WORKSPACE)"
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC" \
    "sherpa download-stt" -- npm run download-stt:en --workspace="$WORKSPACE"
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC" \
    "sherpa download-tts" -- npm run download-tts:en --workspace="$WORKSPACE"

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
  ensure_native_node
  ensure_ts_dist
  bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"
  ensure_sherpa_models

  local total="${#SHERPA_ROUNDTRIP_E2E[@]}"
  local idx=0
  for script in "${SHERPA_ROUNDTRIP_E2E[@]}"; do
    idx=$((idx + 1))
    CI_STEP_INDEX=$idx CI_STEP_TOTAL=$total \
      bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_ROUNDTRIP_TIMEOUT_SEC" \
        "sherpa e2e $script" -- bash "$ROOT/scripts/ci/run-sherpa-roundtrip-e2e.sh" "$script"
  done
}

mode="${1:-all}"
case "$mode" in
  typecheck) run_typecheck ;;
  vitest) run_vitest ;;
  e2e) run_e2e ;;
  all)
    run_typecheck
    run_vitest
    run_e2e
    ;;
  *)
    echo "Usage: $0 {typecheck|vitest|e2e|all}" >&2
    exit 2
    ;;
esac

echo "==> Sherpa example CI ($mode) OK"
