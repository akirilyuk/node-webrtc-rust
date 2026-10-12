#!/usr/bin/env bash
# Guards the native trace-on-failure wiring (error type net.connect-stall-no-timeout):
# - run-pr-integration.sh sets WNR_LOG_FILE / RUST_LOG only around the helpers integration step,
#   keeps the trace dir on failure and deletes it on success;
# - the integration action uploads it as an artifact with `if: failure()` only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/ci/run-pr-integration.sh"
ACTION="$ROOT/.github/actions/ci-run-integration-tests/action.yml"

fail() { echo "native-trace-wiring.test.sh: $*" >&2; exit 1; }
count() { grep -c -E "$1" "$2" || true; }

[[ "$(count '^[^#]*WNR_LOG_FILE=' "$SCRIPT")" == 1 ]] || fail "WNR_LOG_FILE must be set exactly once in $SCRIPT"
[[ "$(count '^[^#]* RUST_LOG=' "$SCRIPT")" == 1 ]] || fail "RUST_LOG must be set exactly once in $SCRIPT"

# Both variables are applied on the one command that runs the helpers integration step.
step="$(grep -n -E '^if WNR_LOG_FILE=.*RUST_LOG=' "$SCRIPT" | cut -d: -f1)"
[[ -n "$step" ]] || fail "WNR_LOG_FILE and RUST_LOG must prefix the helpers integration command"
sed -n "$((step)),$((step + 2))p" "$SCRIPT" | grep -q '"helpers integration"' \
  || fail "the env prefix is not on the helpers integration step"
grep -q 'test:integration --workspace=@node-webrtc-rust/helpers; then' "$SCRIPT" \
  || fail "helpers integration must be the if-condition (trace kept only on failure)"

# Per-process file name, no global level.
grep -q 'WNR_LOG_FILE="\$NATIVE_TRACE_DIR/native-{pid}.log"' "$SCRIPT" || fail "{pid} path missing"
rust_log="$(grep -E '^NATIVE_TRACE_RUST_LOG=' "$SCRIPT" | sed -E 's/^[^"]*"([^"]*)".*/\1/')"
for target in dtls webrtc_dtls webrtc::dtls_transport webrtc_ice webrtc_sctp; do
  [[ ",$rust_log," == *",$target=trace,"* ]] || fail "RUST_LOG lacks $target=trace"
done
[[ ",$rust_log," == *",webrtc::mux=debug,"* ]] || fail "RUST_LOG lacks webrtc::mux=debug (no endpoint for packet warnings)"
[[ "$rust_log" != trace* && "$rust_log" != debug* ]] || fail "RUST_LOG must not start with a global level"

# Delete on success, keep (and exit non-zero) on failure.
grep -q '  rm -rf "\$NATIVE_TRACE_DIR"' "$SCRIPT" || fail "trace dir is not deleted on success"
grep -q 'exit "\$helpers_rc"' "$SCRIPT" || fail "failure must exit with the step's status"

# Artifact upload: failure only, 7 days.
grep -q 'actions/upload-artifact@v4' "$ACTION" || fail "upload-artifact@v4 missing"
up="$(grep -n 'upload-artifact@v4' "$ACTION" | cut -d: -f1)"
sed -n "$((up - 3)),$((up + 8))p" "$ACTION" | grep -q 'if: failure()' || fail "upload must be if: failure()"
grep -q 'name: native-trace-\${{ github.run_id }}' "$ACTION" || fail "artifact name must be native-trace-<run_id>"
grep -q 'path: .test-logs/native-trace' "$ACTION" || fail "artifact path"
grep -q 'retention-days: 7' "$ACTION" || fail "retention-days: 7 missing"

echo "native-trace-wiring.test.sh: OK"
