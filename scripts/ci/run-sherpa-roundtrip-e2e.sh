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
  env \
    CI=true \
    VOICE_DEBUG="$voice_debug" \
    SHERPA_ROUNDTRIP_TOPOLOGY_LOG=0 \
    "${extra_env[@]}" \
    npm run "$SCRIPT" --workspace="$WORKSPACE"
}

if run_pass 0; then
  exit 0
fi

if [[ "$RETRIES" -lt 1 ]]; then
  exit 1
fi

echo "[sherpa-e2e] $SCRIPT failed (VOICE_DEBUG=0) — re-running with VOICE_DEBUG=1" >&2
run_pass 1
