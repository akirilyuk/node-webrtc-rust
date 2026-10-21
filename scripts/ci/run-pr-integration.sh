#!/usr/bin/env bash
# Native + integration tests — requires linux-gnu .node in packages/bindings/.
# Runs inside the ci-test image. The .node is the release artifact the caller staged
# (main / release) or is built here as a debug binding (PRs). TS dist may come from the
# build-ts cache or is built here on cache miss. With TURN_AVAILABLE=1 it starts coturn itself.
#
# CI_TEST_SUITES (comma list of rust,node,sherpa; empty or unset = all) limits the run to the
# suites a PR's changed paths can affect (scripts/ci/pr-test-suites.map via
# compute-pr-job-gates.sh). Only the PR workflow sets it; main, release and local runs run everything.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CI_STEP="$ROOT/scripts/ci/ci-step.sh"
DEFAULT_CARGO_LIB_TIMEOUT_SEC="${CI_CARGO_LIB_TIMEOUT_SEC:-180}"
DEFAULT_CARGO_PEER_TEST_TIMEOUT_SEC="${CI_CARGO_PEER_TEST_TIMEOUT_SEC:-420}"
DEFAULT_NPM_TEST_TIMEOUT_SEC="${CI_NPM_TEST_TIMEOUT_SEC:-600}"
# Budget for compiling every cargo test binary below (cache-dependent: cold cache ~2x warm).
DEFAULT_CARGO_BUILD_TIMEOUT_SEC="${CI_CARGO_BUILD_TIMEOUT_SEC:-900}"

CI_TEST_SUITES="${CI_TEST_SUITES:-}"
suite_enabled() { [[ -z "$CI_TEST_SUITES" || ",$CI_TEST_SUITES," == *",$1,"* ]]; }
echo "==> test suites: ${CI_TEST_SUITES:-rust,node,sherpa (all)}"
for suite in rust node sherpa; do
  if ! suite_enabled "$suite"; then
    echo "::notice::Skipping $suite tests: no changed path maps to them (path-based selection; main runs all suites)"
  fi
done

echo "==> npm ci"
bash scripts/ci/npm-ci-workspace.sh

echo "==> integration git $(git rev-parse --short HEAD)"

if suite_enabled node || suite_enabled sherpa; then
  echo "==> ensure native binding"
  shopt -s nullglob
  nodes=(packages/bindings/*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "    no .node in workspace — building debug x86_64-unknown-linux-gnu binding"
    bash "$CI_STEP" --timeout "${CI_NATIVE_BUILD_TIMEOUT_SEC:-1200}" "native debug build" -- \
      bash "$ROOT/scripts/ci/build-native-addon.sh" --target x86_64-unknown-linux-gnu --profile debug
    shopt -s nullglob
    nodes=(packages/bindings/*.node)
    if [[ ${#nodes[@]} -eq 0 ]]; then
      echo "No .node artifact after native build." >&2
      exit 1
    fi
  else
    echo "    using $(basename "${nodes[0]}") (release artifact or earlier build)"
  fi
fi

if [[ "${TURN_AVAILABLE:-}" == "1" ]] && suite_enabled node; then
  echo "==> TURN server (coturn in this container)"
  bash "$ROOT/scripts/ci/turnserver-ci.sh" start
  trap 'bash "$ROOT/scripts/ci/turnserver-ci.sh" stop' EXIT
fi

if suite_enabled node || suite_enabled sherpa; then
  echo "==> sync workspace bindings for npm test (registry copy under packages/sdk)"
  bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"
fi

if suite_enabled rust; then
  echo "==> Rust fmt + clippy (toolchain pinned in rust-toolchain.toml)"
  bash "$CI_STEP" --timeout "${CI_RUST_LINT_TIMEOUT_SEC:-1500}" "rust lint" -- \
    bash scripts/ci/run-rust-lint.sh

  echo "==> cargo test (core, mixer, conference, speech, cloud TTS vendors)"
  echo "    (compiles Rust test deps on cold cache — separate from the .node binding above)"
  # opus_sdp_fmtp_omits_maxaveragebitrate_when_env_unset uses OnceLock. A developer
  # shell with WEBRTC_OPUS_BITRATE_BPS=192000 (common locally) fails those tests even
  # though GitHub CI is unset. Unset here so release-tag local CI is hermetic.
  # Lesson: development/node-webrtc-rust/features/2026-07-30-opus-sdp-omit-unset.md
  unset WEBRTC_OPUS_BITRATE_BPS
  unset WEBRTC_OPUS_APPLICATION
  # Compile the test binaries in their own step. The per-crate caps below are wall-clock caps on a
  # whole `cargo test`, so they used to include a compile time that depends on the build cache: on
  # release-prep PR #475 the speech step compiled for 76 s on a cold cache, ran its tests for 104 s
  # and hit the 180 s cap (exit 124) although it took 139-143 s on warm runs (2026-10-19,
  # test.time-budget-creep). With the compile separated, the later caps measure test time only, and
  # ci-step.sh warns when any step uses >= 75 % of its cap.
  # Keep this list in lockstep with the cargo steps below: each line is that step's `cargo test`
  # (same -p, --features, --manifest-path and test targets) plus --no-run, so the artifacts are
  # identical (same feature unification) and the later step does not compile again. Guard:
  # scripts/ci/run-pr-integration.test.sh.
  cargo_test_build() {
    set -euo pipefail
    cargo test --no-run -p node-webrtc-rust-core --lib --test peer_connection_test
    cargo test --no-run -p node-webrtc-rust-mixer
    cargo test --no-run -p node-webrtc-rust-conference
    cargo test --no-run -p node-webrtc-rust-speech
    cargo test --no-run -p node-webrtc-rust-vendor-cartesia --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-assemblyai --features live --lib --test mock_server_test --test tls_provider_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-deepgram --features live --lib --test http_pool_test --test mock_server_test --test stt_agent_mock_test --test stt_mock_server_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-elevenlabs --features live --lib --test http_pool_test --test mock_server_test --test stt_mock_server_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-fishaudio --features live --lib --test http_pool_test --test mock_server_test --test stt_mock_server_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-mistral --features live --lib --test live_endpoints --test stt_mock_server_test --test tts_mock_server_test
    cargo test --no-run -p node-webrtc-rust-vendor-azure --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-groq --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-openai --features live --lib --test vendor_test --test http_pool_test --test mock_server_test
    cargo test --no-run -p node-webrtc-rust-vendor-google --features live --lib --test vendor_test --test http_pool_test --test mock_server_test
    cargo test --no-run -p node-webrtc-rust-vendor-aws --features live --lib --test mock_server_test --test vendor_test
    cargo test --no-run -p node-webrtc-rust-vendor-live-support
    cargo test --no-run --manifest-path third_party/dtls/Cargo.toml
    cargo test --no-run --manifest-path third_party/webrtc-ice/Cargo.toml
    cargo check --benches -p node-webrtc-rust-speech -p node-webrtc-rust-vendor-sherpa-onnx -p node-webrtc-rust-core
  }
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_BUILD_TIMEOUT_SEC" "cargo test build" -- \
    bash -c "$(declare -f cargo_test_build); cargo_test_build"

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
  # Streaming TTS vendors: their mock-server tests (one-send-path-per-unit) only run with `live`.
  # Cloud vendors are not live-tested in CI, so these local mock servers are the only guard of the
  # sink contract (each chunk to the sink once, no fallback after audio) and the wait bounds.
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-cartesia" -- \
    cargo test -p node-webrtc-rust-vendor-cartesia --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-assemblyai" -- \
    cargo test -p node-webrtc-rust-vendor-assemblyai --features live --lib --test mock_server_test --test tls_provider_test --test vendor_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-deepgram" -- \
    cargo test -p node-webrtc-rust-vendor-deepgram --features live --lib --test http_pool_test --test mock_server_test --test stt_agent_mock_test --test stt_mock_server_test --test vendor_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-elevenlabs" -- \
    cargo test -p node-webrtc-rust-vendor-elevenlabs --features live --lib --test http_pool_test --test mock_server_test --test stt_mock_server_test --test vendor_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-fishaudio" -- \
    cargo test -p node-webrtc-rust-vendor-fishaudio --features live --lib --test http_pool_test --test mock_server_test --test stt_mock_server_test --test vendor_test -- --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-mistral" -- \
    cargo test -p node-webrtc-rust-vendor-mistral --features live --lib --test live_endpoints --test stt_mock_server_test --test tts_mock_server_test -- --test-threads=1
  # Cloud vendors with a pooled HTTP client (HTTP pool reuse, last_transport, timeout bounds) and
  # their mock-server tests (progressive TTS body, loud STT failures). Test
  # targets are listed instead of a bare `cargo test` so a live_test / live_bench target (they need
  # real keys) never runs here: those run only in vendor-live-*.yml / vendor-bench.yml.
  # Guard: crates/vendor-live-support/tests/workflow_lockstep_test.rs.
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-azure" -- \
    cargo test -p node-webrtc-rust-vendor-azure --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-groq" -- \
    cargo test -p node-webrtc-rust-vendor-groq --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-openai" -- \
    cargo test -p node-webrtc-rust-vendor-openai --features live --lib --test vendor_test --test http_pool_test --test mock_server_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-google" -- \
    cargo test -p node-webrtc-rust-vendor-google --features live --lib --test vendor_test --test http_pool_test --test mock_server_test
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-aws" -- \
    cargo test -p node-webrtc-rust-vendor-aws --features live --lib --test mock_server_test --test vendor_test
  # Shared live-test/bench support: unit tests, the VAD gate test, and the workflow lockstep guard
  # (every live_test.rs has a vendor-live-<id>.yml with the fork-skip, _LIVE_REQUIRED and paths).
  bash "$CI_STEP" --timeout "$DEFAULT_CARGO_LIB_TIMEOUT_SEC" "cargo vendor-live-support" -- \
    cargo test -p node-webrtc-rust-vendor-live-support
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
fi

if suite_enabled node || suite_enabled sherpa; then
  bash "$ROOT/scripts/ci/ensure-ts-dist.sh"
fi

if suite_enabled node; then
  echo "==> npm test"
  bash "$CI_STEP" --timeout "$DEFAULT_NPM_TEST_TIMEOUT_SEC" "npm test" -- npm test

  # Helpers default `npm test` excludes *.integration.test.ts (quality job / leftover .node).
  echo "==> native binding exports match index.d.ts"
  node scripts/ci/check-native-exports.mjs

  echo "==> helpers native integration tests"
  # Native transport trace for failures only. The bindings write one bounded file per vitest worker
  # process ({pid}) when WNR_LOG_FILE is set (packages/bindings/src/native_trace.rs: transport
  # targets only, no customer content). The files are an artifact when this step fails
  # (.github/workflows/reusable-test.yml) and are deleted when it passes.
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
fi

if suite_enabled sherpa; then
  echo "==> Sherpa roundtrip E2E (all start:roundtrip-*)"
  bash scripts/ci/run-sherpa-example-ci.sh e2e
fi

echo "==> cargo target sizes (build cache sizing)"
du -sh target third_party/*/target 2>/dev/null || true
# Cheap breakdown (always): which part of target/debug is worth caching.
for d in deps build .fingerprint incremental; do
  [[ -d "target/debug/$d" ]] && du -sh "target/debug/$d" 2>/dev/null || true
done
if [[ -d target/debug/deps ]]; then
  lib_size="$(find target/debug/deps -maxdepth 1 -type f \( -name '*.rlib' -o -name '*.rmeta' -o -name '*.so' -o -name '*.d' \) -print0 \
    | du -ch --files0-from=- 2>/dev/null | tail -1 | cut -f1 || true)"
  other_size="$(find target/debug/deps -maxdepth 1 -type f ! \( -name '*.rlib' -o -name '*.rmeta' -o -name '*.so' -o -name '*.d' \) -print0 \
    | du -ch --files0-from=- 2>/dev/null | tail -1 | cut -f1 || true)"
  echo "target/debug/deps library artifacts (rlib/rmeta/so/d): ${lib_size:-n/a}"
  echo "target/debug/deps everything else (test/bench executables): ${other_size:-n/a}"
fi
# Compressed size of the snapshot a build cache would hold (libraries + build scripts +
# fingerprints, no test executables, no incremental). Push runs only (or CI_TARGET_SNAPSHOT_PROBE=1)
# so PR runs do not pay for it. Never fails the run.
if [[ "${GITHUB_EVENT_NAME:-}" == "push" || "${CI_TARGET_SNAPSHOT_PROBE:-}" == "1" ]]; then
  if [[ -d target/debug/deps ]]; then
    snapshot_list="$(mktemp)"
    {
      [[ -d target/debug/build ]] && echo "debug/build"
      [[ -d target/debug/.fingerprint ]] && echo "debug/.fingerprint"
      find target/debug -maxdepth 1 -type f -name '*.d' | sed 's#^target/##'
      find target/debug/deps -maxdepth 1 -type f \( -name '*.rlib' -o -name '*.rmeta' -o -name '*.so' -o -name '*.d' \) | sed 's#^target/##'
    } >"$snapshot_list"
    snapshot_out="$(mktemp)"
    snapshot_start=$SECONDS
    # The command writes the byte count to a file: ci-step.sh prints its own banners on stdout.
    bash "$CI_STEP" --timeout 300 "target snapshot probe" -- \
      bash -c 'tar -C target -cf - -T "$1" 2>/dev/null | xz -T0 -1 | wc -c >"$2"' _ "$snapshot_list" "$snapshot_out" || true
    snapshot_bytes="$(tr -dc '0-9' <"$snapshot_out" 2>/dev/null || true)"
    if [[ -n "$snapshot_bytes" ]]; then
      echo "trimmed snapshot (xz -1): $((snapshot_bytes / 1048576)) MiB ($((SECONDS - snapshot_start))s)"
    else
      echo "trimmed snapshot (xz -1): probe failed or timed out ($((SECONDS - snapshot_start))s)"
    fi
    rm -f "$snapshot_out"
    rm -f "$snapshot_list"
  fi
fi

echo "==> Integration tests OK"
