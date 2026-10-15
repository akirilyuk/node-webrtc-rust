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

echo "==> Rust fmt + clippy (toolchain pinned in rust-toolchain.toml)"
bash "$CI_STEP" --timeout "${CI_RUST_LINT_TIMEOUT_SEC:-1500}" "rust lint" -- \
  bash scripts/ci/run-rust-lint.sh

echo "==> cargo test (core, mixer, conference, speech)"
echo "    (compiles Rust test deps on cold cache — separate from the .node binding above)"
# opus_sdp_fmtp_omits_maxaveragebitrate_when_env_unset uses OnceLock. A developer
# shell with WEBRTC_OPUS_BITRATE_BPS=192000 (common locally) fails those tests even
# though GitHub CI is unset. Unset here so release-tag local CI is hermetic.
# Lesson: development/node-webrtc-rust/features/2026-07-30-opus-sdp-omit-unset.md
unset WEBRTC_OPUS_BITRATE_BPS
unset WEBRTC_OPUS_APPLICATION
# WebRTC peer integration tests flake when run in parallel (shared ICE/ports).
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo core --lib" -- \
  cargo test -p node-webrtc-rust-core --lib
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_PEER_TEST_TIMEOUT_SEC" "cargo peer_connection_test" -- \
  cargo test -p node-webrtc-rust-core --test peer_connection_test -- --test-threads=1
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo mixer" -- \
  cargo test -p node-webrtc-rust-mixer
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo conference" -- \
  cargo test -p node-webrtc-rust-conference
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo speech" -- \
  cargo test -p node-webrtc-rust-speech
# Vendored dtls (third_party/dtls) is not a workspace member, so `-p` cannot reach it. Guards
# net.final-flight-not-resent (seeded loss/reorder handshake test).
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendored dtls" -- \
  cargo test --manifest-path third_party/dtls/Cargo.toml
# Vendored webrtc-ice (third_party/webrtc-ice), same reason. Guards net.ice-check-loop-blocked-by-send
# (a STUN write that never completes must not stop the agent's checks and nomination) and the
# nomination failover / bounded data writes (agent_failover_test).
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendored webrtc-ice" -- \
  cargo test --manifest-path third_party/webrtc-ice/Cargo.toml -- agent_stun_send_test agent_failover_test
# Perf benches only compile here; timing thresholds are not a PR gate (scripts/perf/).
# Type-check benches only: `cargo bench --no-run` compiles in the release profile, which CI
# does not cache, and exceeded the 180 s step timeout on a cold cache (#311, #313).
bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo bench check" -- \
  cargo check --benches -p node-webrtc-rust-speech -p node-webrtc-rust-vendor-sherpa-onnx -p node-webrtc-rust-core

bash "$ROOT/scripts/ci/ensure-ts-dist.sh"

echo "==> npm test"
bash "$CI_STEP" --timeout "$DEFAULT_NPM_TEST_TIMEOUT_SEC" "npm test" -- npm test

# Helpers default `npm test` excludes *.integration.test.ts (quality job / leftover .node).
echo "==> native binding exports match index.d.ts"
node scripts/ci/check-native-exports.mjs

echo "==> helpers native integration tests"
# Native transport trace for failures only. The bindings write one bounded file per vitest worker
# process ({pid}) when WNR_LOG_FILE is set (packages/bindings/src/native_trace.rs: transport
# targets only, no customer content). The files are an artifact when this step fails
# (.github/actions/ci-run-integration-tests) and are deleted when it passes.
# Guard: packages/helpers/tests/native-trace-ci-wiring.test.ts (net.connect-stall-no-timeout).
NATIVE_TRACE_DIR="$ROOT/.test-logs/native-trace"
NATIVE_TRACE_RUST_LOG="dtls=trace,webrtc_dtls=trace,webrtc::dtls_transport=trace,webrtc::peer_connection=trace,webrtc::mux=debug,webrtc_ice=trace,webrtc_sctp=trace,webrtc::sctp_transport=trace"
rm -rf "$NATIVE_TRACE_DIR"
mkdir -p "$NATIVE_TRACE_DIR"
if WNR_LOG_FILE="$NATIVE_TRACE_DIR/native-{pid}.log" RUST_LOG="$NATIVE_TRACE_RUST_LOG" \
  bash "$CI_STEP" --timeout "$DEFAULT_NPM_TEST_TIMEOUT_SEC" "helpers integration" -- \
  npm run test:integration --workspace=@node-webrtc-rust/helpers; then
  rm -rf "$NATIVE_TRACE_DIR"
else
  helpers_rc=$?
  echo "helpers integration failed: native trace kept in $NATIVE_TRACE_DIR" >&2
  exit "$helpers_rc"
fi

echo "==> Sherpa roundtrip E2E (all start:roundtrip-*)"
bash scripts/ci/run-sherpa-example-ci.sh e2e

echo "==> Integration tests OK"
