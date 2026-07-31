#!/usr/bin/env bash
# Sherpa local example CI — typecheck, roundtrip Vitest evaluators, and/or Sherpa E2E.
#
# Usage (from repo root):
#   bash scripts/ci/run-sherpa-example-ci.sh typecheck
#   bash scripts/ci/run-sherpa-example-ci.sh vitest    # no models / .node
#   bash scripts/ci/run-sherpa-example-ci.sh e2e       # models + rust ignored TTS cache + roundtrips
#   bash scripts/ci/run-sherpa-example-ci.sh rust      # models + vendor ignored cargo tests only
#   bash scripts/ci/run-sherpa-example-ci.sh all       # typecheck + vitest + e2e
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CI_STEP="$ROOT/scripts/ci/ci-step.sh"
# Per roundtrip script — must exceed in-process SHERPA_ROUNDTRIP_WALL_MS (often 70–120s).
# Long scripts and failure re-runs (VOICE_DEBUG) need headroom — see sherpa_roundtrip_timeout_sec.
DEFAULT_SHERPA_ROUNDTRIP_TIMEOUT_SEC="${CI_SHERPA_ROUNDTRIP_TIMEOUT_SEC:-180}"
DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC="${CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC:-900}"
# Cold compile of vendor-sherpa-onnx + several Piper synths.
DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC="${CI_SHERPA_RUST_IGNORED_TIMEOUT_SEC:-420}"

sherpa_roundtrip_timeout_sec() {
  local script="$1"
  case "$script" in
    start:roundtrip-counting-barge-recovery)
      # 240s in-process wall; quiet pass + VOICE_DEBUG re-run on failure.
      echo "${CI_SHERPA_BARGE_RECOVERY_TIMEOUT_SEC:-420}"
      ;;
    start:roundtrip-counting)
      # 120s in-process wall + long TTS; allow one VOICE_DEBUG re-run.
      echo "${CI_SHERPA_COUNTING_ROUNDTRIP_TIMEOUT_SEC:-300}"
      ;;
    start:roundtrip-counting-echo | start:roundtrip)
      echo "${CI_SHERPA_LONG_ROUNDTRIP_TIMEOUT_SEC:-300}"
      ;;
    *)
      echo "$DEFAULT_SHERPA_ROUNDTRIP_TIMEOUT_SEC"
      ;;
  esac
}

WORKSPACE="@node-webrtc-rust/example-voice-agent-local-sherpa"

# Sherpa roundtrip E2E entry points (see examples/voice-agent-local-sherpa/ROUNDTRIP.md).
SHERPA_ROUNDTRIP_E2E=(
  start:roundtrip-counting
  start:record
  start:roundtrip-utterance-timing
  start:roundtrip-two-phrases
  start:roundtrip-barge-in
  start:roundtrip-barge-in-buffered
  start:roundtrip-counting-echo
  start:roundtrip-counting-barge-recovery
  start:roundtrip-concurrent-multi-client
  start:roundtrip-tts-stream
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
  bash "$ROOT/scripts/ci/ensure-workspace-bindings.sh"
  bash "$ROOT/scripts/ci/ensure-vitest-optional-bindings.sh"
  bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"
  echo "==> roundtrip Vitest evaluators ($WORKSPACE)"
  npm run test:roundtrip-counting --workspace="$WORKSPACE"
}

ensure_sherpa_models() {
  echo "==> Sherpa STT/TTS model weights ($WORKSPACE)"
  # ensure-sherpa-models.sh validates/downloads in a subprocess — re-export paths here.
  CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC="$DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC" \
    SHERPA_EXAMPLE_WORKSPACE="$WORKSPACE" \
    bash "$ROOT/scripts/ci/ensure-sherpa-models.sh"
  unset SHERPA_STT_MODEL_PATH SHERPA_TTS_MODEL_PATH SHERPA_STT_LANGUAGE
  # shellcheck source=/dev/null
  source "$ROOT/scripts/export-sherpa-local-models.sh"
}

# Model-backed vendor tests kept #[ignore] in default cargo test (no weights in PR quality).
run_rust_ignored() {
  if [[ -z "${SHERPA_TTS_MODEL_PATH:-}" ]] || [[ ! -d "$SHERPA_TTS_MODEL_PATH" ]]; then
    echo "SHERPA_TTS_MODEL_PATH missing or not a directory (run ensure_sherpa_models first)." >&2
    echo "  SHERPA_TTS_MODEL_PATH=${SHERPA_TTS_MODEL_PATH:-}" >&2
    exit 1
  fi
  echo "==> cargo ignored TTS phrase cache + stream-chunks tests (SHERPA_TTS_MODEL_PATH=$SHERPA_TTS_MODEL_PATH)"
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust tts_phrase_cache_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_phrase_cache_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust tts_stream_chunks_integration_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_stream_chunks_integration_test -- \
      --ignored --nocapture --test-threads=1
}

run_e2e() {
  ensure_native_node
  ensure_ts_dist
  bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"
  ensure_sherpa_models
  run_rust_ignored

  local total="${#SHERPA_ROUNDTRIP_E2E[@]}"
  local idx=0
  for script in "${SHERPA_ROUNDTRIP_E2E[@]}"; do
    idx=$((idx + 1))
    local step_timeout
    step_timeout="$(sherpa_roundtrip_timeout_sec "$script")"
    CI_STEP_INDEX=$idx CI_STEP_TOTAL=$total \
      bash "$CI_STEP" --timeout "$step_timeout" \
        "sherpa e2e $script" -- bash "$ROOT/scripts/ci/run-sherpa-roundtrip-e2e.sh" "$script"
  done
}

run_rust() {
  ensure_sherpa_models
  run_rust_ignored
}

mode="${1:-all}"
case "$mode" in
  typecheck) run_typecheck ;;
  vitest) run_vitest ;;
  e2e) run_e2e ;;
  rust) run_rust ;;
  all)
    run_typecheck
    run_vitest
    run_e2e
    ;;
  *)
    echo "Usage: $0 {typecheck|vitest|e2e|rust|all}" >&2
    exit 2
    ;;
esac

echo "==> Sherpa example CI ($mode) OK"
