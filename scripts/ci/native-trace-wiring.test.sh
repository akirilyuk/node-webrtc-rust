#!/usr/bin/env bash
# Guards the native trace-on-failure wiring (error type net.connect-stall-no-timeout):
# - run-pr-integration.sh sets WNR_LOG_FILE / RUST_LOG only around the helpers integration step,
#   keeps the trace dir on failure and deletes it on success;
# - the reusable integration workflow (reusable-test.yml) uploads it as an artifact with `if: failure()` only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/ci/run-pr-integration.sh"
WORKFLOW="$ROOT/.github/workflows/reusable-test.yml"

fail() { echo "native-trace-wiring.test.sh: $*" >&2; exit 1; }
count() { grep -c -E "$1" "$2" || true; }

[[ "$(count '^[^#]*WNR_LOG_FILE=' "$SCRIPT")" == 1 ]] || fail "WNR_LOG_FILE must be set exactly once in $SCRIPT"
[[ "$(count '^[^#]* RUST_LOG=' "$SCRIPT")" == 1 ]] || fail "RUST_LOG must be set exactly once in $SCRIPT"

# Both variables are applied on the one command that runs the helpers integration step.
step="$(grep -n -E '^[[:space:]]*if WNR_LOG_FILE=.*RUST_LOG=' "$SCRIPT" | cut -d: -f1)"
[[ -n "$step" ]] || fail "WNR_LOG_FILE and RUST_LOG must prefix the helpers integration command"
sed -n "$((step)),$((step + 2))p" "$SCRIPT" | grep -q '"helpers integration"' \
  || fail "the env prefix is not on the helpers integration step"
grep -q 'test:integration --workspace=@node-webrtc-rust/helpers; then' "$SCRIPT" \
  || fail "helpers integration must be the if-condition (trace kept only on failure)"

# Per-process file name, no global level.
grep -q 'WNR_LOG_FILE="\$NATIVE_TRACE_DIR/native-{pid}.log"' "$SCRIPT" || fail "{pid} path missing"
rust_log="$(grep -E '^[[:space:]]*NATIVE_TRACE_RUST_LOG=' "$SCRIPT" | sed -E 's/^[^"]*"([^"]*)".*/\1/')"
for target in dtls webrtc_dtls webrtc::dtls_transport webrtc_ice webrtc_sctp; do
  [[ ",$rust_log," == *",$target=trace,"* ]] || fail "RUST_LOG lacks $target=trace"
done
[[ ",$rust_log," == *",webrtc::mux=debug,"* ]] || fail "RUST_LOG lacks webrtc::mux=debug (no endpoint for packet warnings)"
[[ "$rust_log" != trace* && "$rust_log" != debug* ]] || fail "RUST_LOG must not start with a global level"

# Delete on success, keep (and exit non-zero) on failure.
grep -q '  rm -rf "\$NATIVE_TRACE_DIR"' "$SCRIPT" || fail "trace dir is not deleted on success"
grep -q 'exit "\$helpers_rc"' "$SCRIPT" || fail "failure must exit with the step's status"

# Artifact upload: failure only, 7 days.
grep -q 'actions/upload-artifact@v4' "$WORKFLOW" || fail "upload-artifact@v4 missing"
# Anchor on the native-trace artifact name: the workflow has more than one upload step.
up="$(grep -n 'name: native-trace-' "$WORKFLOW" | head -n 1 | cut -d: -f1)"
[[ -n "$up" ]] || fail "native-trace upload step missing"
sed -n "$((up - 4)),$((up))p" "$WORKFLOW" | grep -q 'if: failure()' || fail "upload must be if: failure()"
grep -q 'name: native-trace-\${{ github.run_id }}' "$WORKFLOW" || fail "artifact name must be native-trace-<run_id>"
grep -q 'path: .test-logs/native-trace' "$WORKFLOW" || fail "artifact path"
grep -q 'retention-days: 7' "$WORKFLOW" || fail "retention-days: 7 missing"

# Sherpa audio tap: its own failure-only artifact, so a misheard roundtrip turn keeps its audio.
grep -q 'path: .test-logs/sherpa-e2e/audio-tap' "$WORKFLOW" || fail "audio tap upload path missing"
tap="$(grep -n 'name: sherpa-audio-tap-' "$WORKFLOW" | cut -d: -f1)"
[[ -n "$tap" ]] || fail "audio tap artifact name must be sherpa-audio-tap-<run_id>-<attempt>"
sed -n "$((tap - 4)),$((tap))p" "$WORKFLOW" | grep -q 'if: failure()' || fail "audio tap upload must be if: failure()"

echo "native-trace-wiring.test.sh: OK"
