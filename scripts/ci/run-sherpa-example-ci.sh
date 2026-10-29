#!/usr/bin/env bash
# Sherpa local example CI — typecheck, roundtrip Vitest evaluators, and/or Sherpa E2E.
#
# Usage (from repo root):
#   bash scripts/ci/run-sherpa-example-ci.sh typecheck
#   bash scripts/ci/run-sherpa-example-ci.sh vitest    # no models / .node
#   bash scripts/ci/run-sherpa-example-ci.sh e2e       # models + rust ignored TTS cache + roundtrips
#   bash scripts/ci/run-sherpa-example-ci.sh rust      # models + vendor ignored cargo tests only
#   bash scripts/ci/run-sherpa-example-ci.sh all       # typecheck + vitest + e2e
#
# e2e runs the roundtrips in two groups: a parallel group (up to SHERPA_E2E_PARALLEL at a time,
# default 2 when CI=true, else 1), then a serial group of timing-sensitive ones.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CI_STEP="$ROOT/scripts/ci/ci-step.sh"
# Per roundtrip script — must exceed in-process SHERPA_ROUNDTRIP_WALL_MS (often 70–120s).
# Long scripts and failure re-runs (VOICE_DEBUG) need headroom — see sherpa_roundtrip_timeout_sec.
DEFAULT_SHERPA_ROUNDTRIP_TIMEOUT_SEC="${CI_SHERPA_ROUNDTRIP_TIMEOUT_SEC:-180}"
DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC="${CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC:-900}"
# Cold compile of vendor-sherpa-onnx + several Piper synths.
DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC="${CI_SHERPA_RUST_IGNORED_TIMEOUT_SEC:-420}"

sherpa_roundtrip_timeout_sec() {
  local script="$1"
  case "$script" in
    start:roundtrip-counting-barge-recovery)
      # 240s in-process wall; quiet pass + VOICE_DEBUG re-run on failure.
      echo "${CI_SHERPA_BARGE_RECOVERY_TIMEOUT_SEC:-420}"
      ;;
    start:roundtrip-counting)
      # 120s in-process wall + long TTS; allow one VOICE_DEBUG re-run.
      echo "${CI_SHERPA_COUNTING_ROUNDTRIP_TIMEOUT_SEC:-300}"
      ;;
    start:roundtrip-counting-echo | start:roundtrip-counting-echo-lid | start:roundtrip)
      echo "${CI_SHERPA_LONG_ROUNDTRIP_TIMEOUT_SEC:-300}"
      ;;
    start:roundtrip-counting-echo-lid-multi)
      # 240s in-process wall (10 concurrent sessions, local LID + TTS). A starved CI host used to
      # leave the VOICE_DEBUG re-run running past the old 180s cap; the re-run is now clamped to the
      # time left (see run-sherpa-roundtrip-e2e.sh). Guard: roundtrip-ci-step-budget.test.ts.
      echo "${CI_SHERPA_LID_MULTI_ROUNDTRIP_TIMEOUT_SEC:-300}"
      ;;
    start:roundtrip-c1-timeouts)
      # ~75s of real-time phases (10s + 16s no-frame waits); 150s in-process wall, one VOICE_DEBUG re-run.
      echo "${CI_SHERPA_C1_TIMEOUTS_ROUNDTRIP_TIMEOUT_SEC:-300}"
      ;;
    start:roundtrip-slow-final)
      # ~60s of real-time phases (6s pending finals x 3 phases, model loads); 150s in-process wall, one VOICE_DEBUG re-run.
      echo "${CI_SHERPA_SLOW_FINAL_ROUNDTRIP_TIMEOUT_SEC:-300}"
      ;;
    start:roundtrip-noisy-bystander)
      # Two real-time passes of a ~90s scene (420s in-process wall); one VOICE_DEBUG re-run.
      echo "${CI_SHERPA_NOISY_BYSTANDER_ROUNDTRIP_TIMEOUT_SEC:-900}"
      ;;
    start:roundtrip-load-ci)
      # 4 concurrent legs x 2 turns; 60s setup + per-turn TTS/silence/STT budget in-process.
      echo "${CI_SHERPA_LOAD_ROUNDTRIP_TIMEOUT_SEC:-420}"
      ;;
    *)
      echo "$DEFAULT_SHERPA_ROUNDTRIP_TIMEOUT_SEC"
      ;;
  esac
}

WORKSPACE="@node-webrtc-rust/example-voice-agent-local-sherpa"

# Sherpa roundtrip E2E entry points (see examples/voice-agent-local-sherpa/ROUNDTRIP.md), in two groups.
# Every start:roundtrip* / start:record script that runs in CI is in exactly one of them; the guard
# scripts/ci/sherpa-e2e-groups.test.sh checks that against the example's package.json.
#
# SERIAL: timing-sensitive (barge-in, utterance timing, real-time phases) or CPU-heavy (load, 10-session multi). Run one at a time,
# after the parallel group, so nothing else overlaps them.
SHERPA_ROUNDTRIP_E2E_SERIAL=(
  start:roundtrip-utterance-timing
  start:roundtrip-c1-timeouts
  start:roundtrip-slow-final
  start:roundtrip-barge-in
  start:roundtrip-barge-in-buffered
  start:roundtrip-noisy-bystander
  start:roundtrip-counting-barge-recovery
  start:roundtrip-concurrent-multi-client
  start:roundtrip-load-ci
  start:roundtrip-barge-replay
  # 10 concurrent voice sessions in one process: CPU-heavy, so never next to other roundtrips on
  # the 4-CPU job pod (starvation drops/doubles words: features/2026-10-14-voice-starvation-*).
  start:roundtrip-counting-echo-lid-multi
)
# PARALLEL: up to SHERPA_E2E_PARALLEL at a time (default 2 when CI=true, else 1). They are mostly
# real-time audio, so the CPU is idle most of the time. Each uses an in-process loopback on port 0 and
# writes only to paths unique to it (see the shared-state notes in scripts/ci/README.md).
SHERPA_ROUNDTRIP_E2E_PARALLEL=(
  start:roundtrip-counting
  start:record
  start:roundtrip-doubled-first-word
  start:roundtrip-two-phrases
  start:roundtrip-counting-echo
  start:roundtrip-counting-echo-lid
  start:roundtrip-language-id
  start:roundtrip-tts-stream
  start:roundtrip
)

ensure_ts_dist() {
  bash "$ROOT/scripts/ci/ensure-ts-dist.sh"
}

ensure_native_node() {
  shopt -s nullglob
  local nodes=(packages/bindings/*.node)
  if [[ ${#nodes[@]} -eq 0 ]]; then
    echo "No native .node in packages/bindings — required for Sherpa E2E." >&2
    echo "Run: npm run build:native" >&2
    exit 1
  fi
}

run_typecheck() {
  ensure_ts_dist
  echo "==> typecheck ($WORKSPACE)"
  npm run typecheck --workspace="$WORKSPACE"
}

run_vitest() {
  ensure_ts_dist
  bash "$ROOT/scripts/ci/ensure-workspace-bindings.sh"
  bash "$ROOT/scripts/ci/ensure-vitest-optional-bindings.sh"
  bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"
  echo "==> roundtrip Vitest evaluators ($WORKSPACE)"
  npm run test:roundtrip-counting --workspace="$WORKSPACE"
}

ensure_sherpa_models() {
  echo "==> Sherpa STT/TTS model weights ($WORKSPACE)"
  # ensure-sherpa-models.sh validates/downloads in a subprocess — re-export paths here.
  CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC="$DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC" \
    SHERPA_EXAMPLE_WORKSPACE="$WORKSPACE" \
    bash "$ROOT/scripts/ci/ensure-sherpa-models.sh"
  unset SHERPA_STT_MODEL_PATH SHERPA_TTS_MODEL_PATH SHERPA_STT_LANGUAGE
  # shellcheck source=/dev/null
  source "$ROOT/scripts/export-sherpa-local-models.sh"
  ensure_speaker_fixtures
}

# Speaker model, Piper voices and the generated conversation scenes for start:roundtrip-noisy-bystander,
# once per job: only the two S3 test scenes the roundtrip plays (-15 dB bystander, caller at the
# default level and at -26 dBFS), with every turn gap raised to 2.5 s so the agent's default
# end-of-turn (1.5 s finalize + VAD hangover) closes each turn before the next speaker starts.
# Scenes are never committed.
ensure_speaker_fixtures() {
  sherpa_e2e_selected start:roundtrip-noisy-bystander || return 0
  local ex="$ROOT/examples/voice-agent-local-sherpa"
  echo "==> speaker assets + noisy-bystander scenes"
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC" \
    "sherpa download-speaker" -- npm run download-speaker --workspace="$WORKSPACE"
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC" \
    "sherpa speaker_fixtures" -- \
    cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
      --models "$ex/.models/speaker" --main-tts "$ex/.models/vits-piper-en_US-amy-low" \
      --phrases "$ex/stt-phrase-qualification.json" --out "$ex/.models/speaker-fixtures" \
      --main-levels -26 --min-turn-gap-ms 2500 --bystander-db -15 --only s3_test_m15
}

# Model-backed vendor tests kept #[ignore] in default cargo test (no weights in PR quality).
run_rust_ignored() {
  if [[ -z "${SHERPA_TTS_MODEL_PATH:-}" ]] || [[ ! -d "$SHERPA_TTS_MODEL_PATH" ]]; then
    echo "SHERPA_TTS_MODEL_PATH missing or not a directory (run ensure_sherpa_models first)." >&2
    echo "  SHERPA_TTS_MODEL_PATH=${SHERPA_TTS_MODEL_PATH:-}" >&2
    exit 1
  fi
  local lid_dir="${SHERPA_LID_MODEL_PATH:-$ROOT/examples/voice-agent-local-sherpa/.models/sherpa-onnx-whisper-tiny}"
  if [[ ! -d "$lid_dir" ]]; then
    echo "SHERPA_LID_MODEL_PATH missing or not a directory (run ensure_sherpa_models first)." >&2
    echo "  SHERPA_LID_MODEL_PATH=$lid_dir" >&2
    exit 1
  fi
  export SHERPA_LID_MODEL_PATH="$lid_dir"
  echo "==> cargo ignored TTS stream-chunks + cancel-not-cached tests (SHERPA_TTS_MODEL_PATH=$SHERPA_TTS_MODEL_PATH)"
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust tts_stream_chunks_integration_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_stream_chunks_integration_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust tts_cancel_not_cached_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_cancel_not_cached_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust tts_sink_chunks_bounded_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_sink_chunks_bounded_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust tts_sentence_scheduling_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_sentence_scheduling_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust lid_stt_parallel_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test lid_stt_parallel_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust lid_shared_model_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test lid_shared_model_test -- \
      --ignored --nocapture --test-threads=1
  STOP_OPEN_ITERATIONS=1 bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust stop_open_utterance_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stop_open_utterance_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust stt_pre_roll_onset_replay_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stt_pre_roll_onset_replay_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust stt_finalize_last_word_test" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stt_finalize_last_word_test -- \
      --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust stt_concurrent_equals_serial_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stt_concurrent_equals_serial_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust stt_push_and_poll_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stt_push_and_poll_test -- \
      --ignored --nocapture --test-threads=1
  bash "$CI_STEP" --timeout "$DEFAULT_SHERPA_RUST_IGNORED_TIMEOUT_SEC" \
    "sherpa rust session_churn_pool_test --ignored" -- \
    cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test session_churn_pool_test -- \
      --ignored --nocapture --test-threads=1
}

# Parallel width: SHERPA_E2E_PARALLEL, default 2 on CI and 1 elsewhere. 2 keeps about 2 cores per
# roundtrip on the 4-CPU job pod: with ~1 core left (3 cores busy) the two-agent pipeline falls behind
# and STT mishears ("I am done speaking" -> "Amdon speaking", reproduced 2026-10-23 with 3 burned
# cores; 30/30 green with 2 burned; width 3 failed 1 of 39 parallel-group runs). Width 1 runs serially with
# the plain step output.
sherpa_e2e_parallel_width() {
  local width="${SHERPA_E2E_PARALLEL:-}"
  if [[ -z "$width" ]]; then
    if [[ "${CI:-}" == "true" ]]; then width=2; else width=1; fi
  fi
  if ! [[ "$width" =~ ^[1-9][0-9]*$ ]]; then
    echo "SHERPA_E2E_PARALLEL must be a positive integer, got: $width" >&2
    exit 2
  fi
  echo "$width"
}

# Test hook: SHERPA_E2E_ONLY="start:record,start:roundtrip" keeps only those entries (both groups).
sherpa_e2e_selected() {
  local script="$1"
  [[ -z "${SHERPA_E2E_ONLY:-}" || ",${SHERPA_E2E_ONLY}," == *",${script},"* ]]
}

# One roundtrip through ci-step.sh with its per-script cap. Test hook: SHERPA_E2E_DRY_COMMAND is a
# bash -c script that replaces the real roundtrip; it gets the npm script name as $1.
run_one_roundtrip() {
  local script="$1" idx="$2" total="$3"
  local step_timeout
  step_timeout="$(sherpa_roundtrip_timeout_sec "$script")"
  local cmd=(bash "$ROOT/scripts/ci/run-sherpa-roundtrip-e2e.sh" "$script")
  if [[ -n "${SHERPA_E2E_DRY_COMMAND:-}" ]]; then
    cmd=(bash -c "$SHERPA_E2E_DRY_COMMAND" sherpa-e2e-dry "$script")
  fi
  SHERPA_ROUNDTRIP_STEP_TIMEOUT_SEC="$step_timeout" CI_STEP_INDEX=$idx CI_STEP_TOTAL=$total \
    bash "$CI_STEP" --timeout "$step_timeout" \
      "sherpa e2e $script" -- "${cmd[@]}" || return $?
  # Barge-in also runs with the opt-in adaptive VAD (same assertions, same step budget).
  if [[ "$script" == "start:roundtrip-barge-in" ]]; then
    SHERPA_VAD_PROVIDER=adaptive SHERPA_ROUNDTRIP_STEP_TIMEOUT_SEC="$step_timeout" \
      CI_STEP_INDEX=$idx CI_STEP_TOTAL=$total \
      bash "$CI_STEP" --timeout "$step_timeout" \
        "sherpa e2e $script (adaptive VAD)" -- "${cmd[@]}"
  fi
}

# Kill a pool member and everything below it (the roundtrip is npm > tsx > node, under timeout).
sherpa_e2e_kill_tree() {
  local pid="$1" child
  if command -v pgrep >/dev/null 2>&1; then
    for child in $(pgrep -P "$pid" 2>/dev/null || true); do
      sherpa_e2e_kill_tree "$child"
    done
  fi
  kill "$pid" 2>/dev/null || true
}

# Pool state (globals: the helpers below and the EXIT trap read it).
POOL_PIDS=()
POOL_NAMES=()
POOL_DONE=()
POOL_FAILED=()
POOL_RUNNING=0
POOL_LOG_DIR=""

# Print the block of every member that finished since the last call. A member is finished when its
# status file exists (written last, atomically) or when its process is gone without one (killed).
pool_report_finished() {
  local k status name
  for k in "${!POOL_PIDS[@]}"; do
    [[ "${POOL_DONE[$k]}" == 1 ]] && continue
    name="${POOL_NAMES[$k]}"
    if [[ -f "$POOL_LOG_DIR/$name.status" ]]; then
      status="$(cat "$POOL_LOG_DIR/$name.status")"
    elif ! kill -0 "${POOL_PIDS[$k]}" 2>/dev/null; then
      status="killed"
    else
      continue
    fi
    POOL_DONE[$k]=1
    POOL_RUNNING=$((POOL_RUNNING - 1))
    echo "==> [sherpa e2e parallel] $name (exit $status)"
    cat "$POOL_LOG_DIR/$name.log" 2>/dev/null || true
    if [[ "$status" != "0" ]]; then
      POOL_FAILED+=("$name (exit $status)")
    fi
  done
}

# EXIT trap while a pool runs: never leave background roundtrips behind.
pool_cleanup() {
  local k
  for k in ${POOL_PIDS[@]+"${!POOL_PIDS[@]}"}; do
    [[ "${POOL_DONE[$k]}" == 1 ]] && continue
    sherpa_e2e_kill_tree "${POOL_PIDS[$k]}"
  done
  wait 2>/dev/null || true
}

# Run the given scripts with at most $width at a time. Each roundtrip writes its combined output to
# .test-logs/sherpa-e2e/<script>.log; when it finishes, its header and whole log are printed in one
# block, so the CI output never interleaves. A failure does not stop the others; the pool drains and
# then the failing names are reported and the function returns 1.
run_roundtrips_parallel() {
  local width="$1" idx_base="$2" total="$3"
  shift 3
  local scripts=("$@")

  if [[ "$width" -gt 1 ]] && (( BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 3) )); then
    echo "SHERPA_E2E_PARALLEL>1 needs bash >= 4.3 (wait -n); this is bash $BASH_VERSION" >&2
    return 2
  fi

  POOL_LOG_DIR="${SHERPA_E2E_LOG_DIR:-$ROOT/.test-logs/sherpa-e2e}"
  mkdir -p "$POOL_LOG_DIR"
  POOL_PIDS=()
  POOL_NAMES=()
  POOL_DONE=()
  POOL_FAILED=()
  POOL_RUNNING=0
  trap pool_cleanup EXIT

  local i script
  for i in "${!scripts[@]}"; do
    script="${scripts[$i]}"
    while [[ "$POOL_RUNNING" -ge "$width" ]]; do
      wait -n 2>/dev/null || true
      pool_report_finished
    done
    rm -f "$POOL_LOG_DIR/$script.status"
    (
      rc=0
      run_one_roundtrip "$script" "$((idx_base + i + 1))" "$total" >"$POOL_LOG_DIR/$script.log" 2>&1 || rc=$?
      echo "$rc" >"$POOL_LOG_DIR/$script.status.tmp"
      mv "$POOL_LOG_DIR/$script.status.tmp" "$POOL_LOG_DIR/$script.status"
    ) &
    POOL_PIDS+=("$!")
    POOL_NAMES+=("$script")
    POOL_DONE+=(0)
    POOL_RUNNING=$((POOL_RUNNING + 1))
  done
  while [[ "$POOL_RUNNING" -gt 0 ]]; do
    wait -n 2>/dev/null || true
    pool_report_finished
  done
  wait 2>/dev/null || true
  trap - EXIT

  if [[ ${#POOL_FAILED[@]} -gt 0 ]]; then
    echo "==> [sherpa e2e parallel] FAILED (${#POOL_FAILED[@]}/${#scripts[@]}): ${POOL_FAILED[*]}" >&2
    return 1
  fi
  return 0
}

run_e2e() {
  ensure_native_node
  ensure_ts_dist
  bash "$ROOT/scripts/ci/sync-workspace-bindings.sh"
  ensure_sherpa_models
  run_rust_ignored
  run_roundtrips
}

# The roundtrips: the parallel group first, then the serial (timing-sensitive) group on its own.
run_roundtrips() {
  local width script idx=0
  width="$(sherpa_e2e_parallel_width)"
  local parallel=() serial=()
  for script in ${SHERPA_ROUNDTRIP_E2E_PARALLEL[@]+"${SHERPA_ROUNDTRIP_E2E_PARALLEL[@]}"}; do
    sherpa_e2e_selected "$script" && parallel+=("$script")
  done
  for script in ${SHERPA_ROUNDTRIP_E2E_SERIAL[@]+"${SHERPA_ROUNDTRIP_E2E_SERIAL[@]}"}; do
    sherpa_e2e_selected "$script" && serial+=("$script")
  done
  local total=$(( ${#parallel[@]} + ${#serial[@]} ))

  if [[ ${#parallel[@]} -gt 0 ]]; then
    if [[ "$width" -gt 1 ]]; then
      echo "==> Sherpa roundtrips, parallel group (${#parallel[@]} scripts, up to $width at a time)"
      run_roundtrips_parallel "$width" 0 "$total" "${parallel[@]}" || return 1
    else
      echo "==> Sherpa roundtrips, parallel group run serially (SHERPA_E2E_PARALLEL=1)"
      for script in "${parallel[@]}"; do
        idx=$((idx + 1))
        run_one_roundtrip "$script" "$idx" "$total"
      done
    fi
  fi
  idx=${#parallel[@]}
  if [[ ${#serial[@]} -gt 0 ]]; then
    echo "==> Sherpa roundtrips, serial group (${#serial[@]} scripts, timing-sensitive)"
    for script in "${serial[@]}"; do
      idx=$((idx + 1))
      run_one_roundtrip "$script" "$idx" "$total"
    done
  fi
}

run_rust() {
  ensure_sherpa_models
  run_rust_ignored
}

mode="${1:-all}"
case "$mode" in
  step-timeouts)
    # "<npm script> <step cap seconds>" per roundtrip E2E step (read by roundtrip-ci-step-budget.test.ts).
    for script in "${SHERPA_ROUNDTRIP_E2E_SERIAL[@]}" "${SHERPA_ROUNDTRIP_E2E_PARALLEL[@]}"; do
      echo "$script $(sherpa_roundtrip_timeout_sec "$script")"
    done
    exit 0
    ;;
  groups)
    # "<serial|parallel> <npm script>" per roundtrip E2E step (read by sherpa-e2e-groups.test.sh).
    for script in "${SHERPA_ROUNDTRIP_E2E_SERIAL[@]}"; do echo "serial $script"; done
    for script in "${SHERPA_ROUNDTRIP_E2E_PARALLEL[@]}"; do echo "parallel $script"; done
    exit 0
    ;;
  parallel-width)
    # Test hook: the width the parallel group would use with the current env.
    sherpa_e2e_parallel_width
    exit 0
    ;;
  roundtrips)
    # Test hook: only the roundtrip phase (no models, no .node check, no Rust ignored tests).
    run_roundtrips
    ;;
  typecheck) run_typecheck ;;
  vitest) run_vitest ;;
  e2e) run_e2e ;;
  rust) run_rust ;;
  all)
    run_typecheck
    run_vitest
    run_e2e
    ;;
  *)
    echo "Usage: $0 {typecheck|vitest|e2e|rust|all|step-timeouts|groups|parallel-width|roundtrips}" >&2
    exit 2
    ;;
esac

echo "==> Sherpa example CI ($mode) OK"
