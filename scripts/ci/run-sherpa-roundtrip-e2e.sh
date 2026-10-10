#!/usr/bin/env bash
# Run one Sherpa roundtrip npm script for CI: [speech] events on, [voice-debug] off.
# On failure, re-run once with VOICE_DEBUG=1 (matches scripts/ci/README.md).
#
# Usage:
#   bash scripts/ci/run-sherpa-roundtrip-e2e.sh start:roundtrip-counting
#
# Env (optional — set automatically when unset):
#   SHERPA_STT_MODEL_PATH, SHERPA_TTS_MODEL_PATH, SHERPA_STT_LANGUAGE
#   SHERPA_ROUNDTRIP_WORKSPACE — default @node-webrtc-rust/example-voice-agent-local-sherpa
#   SHERPA_ROUNDTRIP_E2E_RETRIES — default 1 (one debug re-run after first failure)
#   SHERPA_ROUNDTRIP_STEP_TIMEOUT_SEC — the CI step cap this run lives under (set by
#     run-sherpa-example-ci.sh). The debug re-run gets only the time left under it
#     (SHERPA_ROUNDTRIP_WALL_MAX_MS) and is skipped when too little is left, so a slow first
#     pass can no longer push the re-run past the step cap (exit 124 hid the real failure).
#   SHERPA_ROUNDTRIP_RERUN_MIN_SEC — minimum time left to bother with the re-run (default 45)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${1:?Sherpa roundtrip npm script (e.g. start:roundtrip-counting)}"
WORKSPACE="${SHERPA_ROUNDTRIP_WORKSPACE:-@node-webrtc-rust/example-voice-agent-local-sherpa}"
RETRIES="${SHERPA_ROUNDTRIP_E2E_RETRIES:-1}"

# shellcheck source=/dev/null
source "$ROOT/scripts/export-sherpa-local-models.sh"

run_pass() {
  local voice_debug="${1:?}"
  local extra_env=()
  if [[ "$SCRIPT" == *barge-recovery* ]]; then
    # Linux CI STT partials lag macOS; barge slightly earlier than local default (400ms).
    extra_env+=(SHERPA_BARGE_RECOVERY_DELAY_MS="${SHERPA_BARGE_RECOVERY_DELAY_MS:-350}")
  fi
  if [[ "$SCRIPT" == *barge-in* ]]; then
    # Linux CI: partial→barge_in can lag; agent_speaking_end still truncates playback.
    extra_env+=(SHERPA_BARGE_IN_MAX_RATIO="${SHERPA_BARGE_IN_MAX_RATIO:-0.80}")
  fi
  env \
    CI=true \
    SHERPA_ROUNDTRIP_CI_HARNESS_POST_SILENCE=1 \
    VOICE_DEBUG="$voice_debug" \
    SHERPA_ROUNDTRIP_TOPOLOGY_LOG=0 \
    ${extra_env[@]+"${extra_env[@]}"} \
    npm run "$SCRIPT" --workspace="$WORKSPACE"
}

STARTED_AT=$SECONDS

if run_pass 0; then
  exit 0
fi

if [[ "$RETRIES" -lt 1 ]]; then
  exit 1
fi

STEP_CAP_SEC="${SHERPA_ROUNDTRIP_STEP_TIMEOUT_SEC:-}"
if [[ -n "$STEP_CAP_SEC" ]]; then
  # Keep 10 s for npm/tsx startup and teardown outside the in-process clock.
  left_sec=$((STEP_CAP_SEC - (SECONDS - STARTED_AT) - 10))
  if [[ "$left_sec" -lt "${SHERPA_ROUNDTRIP_RERUN_MIN_SEC:-45}" ]]; then
    echo "[sherpa-e2e] $SCRIPT failed (VOICE_DEBUG=0); only ${left_sec}s left under the ${STEP_CAP_SEC}s step cap — skipping the VOICE_DEBUG=1 re-run" >&2
    exit 1
  fi
  export SHERPA_ROUNDTRIP_WALL_MAX_MS=$((left_sec * 1000))
fi

echo "[sherpa-e2e] $SCRIPT failed (VOICE_DEBUG=0) — re-running with VOICE_DEBUG=1" >&2
run_pass 1
