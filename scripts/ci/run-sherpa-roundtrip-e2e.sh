#!/usr/bin/env bash
# Run one Sherpa roundtrip npm script for CI: [speech] events on, [voice-debug] off.
# On failure, re-run once with VOICE_DEBUG=1 for diagnostics; the step fails either way (matches scripts/ci/README.md).
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
#   SHERPA_E2E_AUDIO_TAP — default on; 0 disables the test audio tap. Each pass sets
#     SPEECH_TEST_AUDIO_TAP_DIR=.test-logs/sherpa-e2e/audio-tap/<script>-pass1|pass2, so a failed
#     pass keeps the listener's inbound audio (inbound-*.wav) and the audio pushed to STT
#     (stt-in-*.wav). A passing pass deletes its directory. Replay a file with
#     `cargo run -p node-webrtc-rust-vendor-sherpa-onnx --example stt_replay -- "$SHERPA_STT_MODEL_PATH" <file.wav>`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${1:?Sherpa roundtrip npm script (e.g. start:roundtrip-counting)}"
WORKSPACE="${SHERPA_ROUNDTRIP_WORKSPACE:-@node-webrtc-rust/example-voice-agent-local-sherpa}"
RETRIES="${SHERPA_ROUNDTRIP_E2E_RETRIES:-1}"

# shellcheck source=/dev/null
source "$ROOT/scripts/export-sherpa-local-models.sh"
# shellcheck source=scripts/ci/ensure-speaker-fixtures.sh
source "$ROOT/scripts/ci/ensure-speaker-fixtures.sh"

# Every path that runs the noisy-bystander roundtrip (run-sherpa-example-ci.sh, the stress workflow,
# local runs) gets its generated scenes here; idempotent, skips when they match the args.
if [[ "$SCRIPT" == "start:roundtrip-noisy-bystander" ]]; then
  ensure_speaker_fixtures
fi

run_pass() {
  local voice_debug="${1:?}"
  local pass="${2:?pass label (pass1|pass2)}"
  local extra_env=()
  local tap_dir=""
  if [[ "${SHERPA_E2E_AUDIO_TAP:-1}" != "0" ]]; then
    tap_dir="$ROOT/.test-logs/sherpa-e2e/audio-tap/${SCRIPT#start:}-$pass"
    rm -rf "$tap_dir" && mkdir -p "$tap_dir"
    extra_env+=(SPEECH_TEST_AUDIO_TAP_DIR="$tap_dir")
  fi
  if [[ "$SCRIPT" == *barge-recovery* ]]; then
    # Linux CI STT partials lag macOS; barge slightly earlier than local default (400ms).
    extra_env+=(SHERPA_BARGE_RECOVERY_DELAY_MS="${SHERPA_BARGE_RECOVERY_DELAY_MS:-350}")
  fi
  if [[ "$SCRIPT" == *barge-in* ]]; then
    # Linux CI: partial→barge_in can lag; agent_speaking_end still truncates playback.
    extra_env+=(SHERPA_BARGE_IN_MAX_RATIO="${SHERPA_BARGE_IN_MAX_RATIO:-0.80}")
  fi
  local rc=0
  env \
    CI=true \
    SHERPA_ROUNDTRIP_CI_HARNESS_POST_SILENCE=1 \
    VOICE_DEBUG="$voice_debug" \
    SHERPA_ROUNDTRIP_TOPOLOGY_LOG=0 \
    ${extra_env[@]+"${extra_env[@]}"} \
    npm run "$SCRIPT" --workspace="$WORKSPACE" || rc=$?
  if [[ -n "$tap_dir" && "$rc" -eq 0 ]]; then
    rm -rf "$tap_dir"
  fi
  return "$rc"
}

STARTED_AT=$SECONDS

# SHERPA_ROUNDTRIP_FIRST_PASS_VOICE_DEBUG=1: the first pass already runs with VOICE_DEBUG=1, so a
# failing pass has [voice-debug] internals (stress investigations; CI keeps the quiet first pass).
if run_pass "${SHERPA_ROUNDTRIP_FIRST_PASS_VOICE_DEBUG:-0}" pass1; then
  exit 0
fi
if [[ "${SHERPA_E2E_AUDIO_TAP:-1}" != "0" ]]; then
  echo "[sherpa-e2e] audio tap kept: $ROOT/.test-logs/sherpa-e2e/audio-tap/${SCRIPT#start:}-pass1" >&2
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

echo "[sherpa-e2e] $SCRIPT failed (VOICE_DEBUG=0) — re-running with VOICE_DEBUG=1 for diagnostics" >&2
# The re-run only collects [voice-debug] output. The step fails either way: a pass here would turn
# a flake into green (flaky-tests-root-cause.md).
if run_pass 1 pass2; then
  echo "[sherpa-e2e] $SCRIPT passed only on the VOICE_DEBUG=1 re-run: nondeterministic, failing the step" >&2
fi
exit 1
