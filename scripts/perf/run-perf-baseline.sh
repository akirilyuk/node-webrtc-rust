#!/usr/bin/env bash
# Layer A perf baseline for node-webrtc-rust (speech perf plan, phase 0).
#
#   bash scripts/perf/run-perf-baseline.sh <label>
#
# Runs the micro benches, collects every `perf_probe {json}` line and the criterion medians into
# `.test-logs/perf/<label>-<git sha>.json` (compare two of them with scripts/perf/compare-perf.mjs).
#
#   criterion benches  inbound_frame, resample, opus_encode   (run once, criterion samples itself)
#   probe benches      tts_drain, idle_agents                 (PERF_PROBE_RUNS times, default 5;
#                                                              median + spread recorded)
#
# Not a PR gate: timings on shared runners are noisy. Run on a quiet machine, same machine for
# base and head. Full bench output goes to .test-logs/<stamp>-perf-bench.log.
set -euo pipefail

LABEL="${1:?usage: run-perf-baseline.sh <label>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SHA="$(git rev-parse --short HEAD)"
export PERF_SHA="$SHA"
RUNS="${PERF_PROBE_RUNS:-5}"
STAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p .test-logs/perf
LOG=".test-logs/${STAMP}-perf-bench.log"
OUT=".test-logs/perf/${LABEL}-${SHA}.json"

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
rm -rf "$TARGET_DIR/criterion"

PKGS=(-p node-webrtc-rust-speech -p node-webrtc-rust-vendor-sherpa-onnx -p node-webrtc-rust-core)

echo "==> criterion benches (log: $LOG)"
cargo bench "${PKGS[@]}" \
  --bench inbound_frame --bench resample --bench opus_encode \
  -- --noplot >"$LOG" 2>&1

for ((i = 1; i <= RUNS; i++)); do
  echo "==> probe benches, run $i/$RUNS"
  cargo bench -p node-webrtc-rust-speech --bench tts_drain >>"$LOG" 2>&1
  cargo bench -p node-webrtc-rust-speech --bench idle_agents >>"$LOG" 2>&1
done

node scripts/perf/collect-perf.mjs "$LOG" "$TARGET_DIR/criterion" "$LABEL" "$SHA" "$OUT"
echo "baseline: $OUT"
