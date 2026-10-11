#!/usr/bin/env bash
# Native + integration tests — requires linux-gnu .node in packages/bindings/.
# Expects quality to have passed. Native .node and TS dist may come from CI caches
# (compile-native / build-ts jobs) or are built here on cache miss.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CI_STEP="$ROOT/scripts/ci/ci-step.sh"
DEFAULT_CARGO_LIB_TIMEOUT_SEC="${CI_CARGO_LIB_TIMEOUT_SEC:-180}"
DEFAULT_CARGO_PEER_TEST_TIMEOUT_SEC="${CI_CARGO_PEER_TEST_TIMEOUT_SEC:-420}"
DEFAULT_NPM_TEST_TIMEOUT_SEC="${CI_NPM_TEST_TIMEOUT_SEC:-600}"

echo "==> npm ci"
bash scripts/ci/npm-ci-workspace.sh

echo "==> integration git $(git rev-parse --short HEAD)"

echo "==> ensure native binding"
shopt -s nullglob
nodes=(packages/bindings/*.node)
if [[ ${#nodes[@]} -eq 0 ]]; then
  if [[ "${CI:-}" == "true" ]]; then
    echo "No .node in CI workspace — compile-native artifact or cache should have restored it." >&2
    exit 1
  fi
  echo "    no .node in workspace — compiling debug linux-gnu binding (local fallback)"
  ( cd packages/bindings && npx napi build --target x86_64-unknown-linux-gnu && npm run copy:local-node )
  shopt -s nullglob
  nodes=(packages/bindings/*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "No .node artifact after fallback compile." >&2
    exit 1
  fi
else
  echo "    using $(basename "${nodes[0]}") (artifact or cache)"
fi

echo "==> sync workspace bindings for npm test (registry copy under packages/sdk)"
bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"

# TEMP (do not merge): trace loop for the mix-three-client DTLS stall.
# Skipped on this branch: cargo test (core/mixer/conference/speech/bench), npm test, Sherpa roundtrip E2E.
bash "$ROOT/scripts/ci/ensure-ts-dist.sh"
ART="$ROOT/trace-artifacts"
mkdir -p "$ART"
export RUST_LOG="dtls=trace,webrtc_ice=debug,webrtc=debug"
TRACE_LOOP_N="${TRACE_LOOP_N:-40}"
fails=0
for i in $(seq 1 "$TRACE_LOOP_N"); do
  nlog="$ART/native-$i.log"
  jslog="$ART/js-$i.log"
  export WNR_LOG_FILE="$nlog"
  rm -f "$nlog"
  echo "==> trace iteration $i/$TRACE_LOOP_N"
  if timeout 300 npm run test:integration --workspace=@node-webrtc-rust/helpers >"$jslog" 2>&1; then
    echo "    iteration $i ok"
    rm -f "$nlog" "$jslog"
  else
    echo "    iteration $i FAILED (logs kept)"
    fails=$((fails + 1))
  fi
done
unset WNR_LOG_FILE
echo "TRACE_LOOP_SUMMARY iterations=$TRACE_LOOP_N failures=$fails"
echo "iterations=$TRACE_LOOP_N failures=$fails" > "$ART/summary.txt"
if [[ "$fails" -gt 0 ]]; then
  exit 1
fi

echo "==> Integration tests OK"
