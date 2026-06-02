#!/usr/bin/env bash
# Run one Sherpa roundtrip npm script for CI: [speech] events on, [voice-debug] off.
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

exec env \
  CI=true \
  VOICE_DEBUG=0 \
  SHERPA_ROUNDTRIP_TOPOLOGY_LOG=0 \
  npm run "$SCRIPT" --workspace="$WORKSPACE"
