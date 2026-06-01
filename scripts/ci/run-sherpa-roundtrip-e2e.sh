#!/usr/bin/env bash
# Run one Sherpa roundtrip npm script for CI: stream [speech] events, suppress [voice-debug]
# on success; re-run with VOICE_DEBUG=1 on failure.
#
# Usage:
#   bash scripts/ci/run-sherpa-roundtrip-e2e.sh start:roundtrip-counting
#
# Env (set by caller):
#   SHERPA_STT_MODEL_PATH, SHERPA_TTS_MODEL_PATH, SHERPA_STT_LANGUAGE
#   SHERPA_ROUNDTRIP_WORKSPACE — default @node-webrtc-rust/example-voice-agent-local-sherpa
set -euo pipefail

SCRIPT="${1:?Sherpa roundtrip npm script (e.g. start:roundtrip-counting)}"
WORKSPACE="${SHERPA_ROUNDTRIP_WORKSPACE:-@node-webrtc-rust/example-voice-agent-local-sherpa}"

quiet_env=(
  CI=true
  VOICE_DEBUG=0
  SHERPA_ROUNDTRIP_TOPOLOGY_LOG=0
)

if env "${quiet_env[@]}" npm run "$SCRIPT" --workspace="$WORKSPACE"; then
  exit 0
fi

echo "" >&2
echo "=== ${SCRIPT} re-run with VOICE_DEBUG=1 ===" >&2
env \
  CI=true \
  VOICE_DEBUG=1 \
  SHERPA_ROUNDTRIP_DEBUG=1 \
  SHERPA_ROUNDTRIP_TOPOLOGY_LOG=1 \
  npm run "$SCRIPT" --workspace="$WORKSPACE" >&2
exit 1
