#!/usr/bin/env bash
# Speech engine cost probes in a CPU-limited container (TTS cost per voice, STT streams per CPU).
#
#   PROBE_MODELS_DIR=examples/voice-agent-local-sherpa/.models \
#     bash scripts/perf/run-voice-cost.sh [--cpus N] [--runs N] [--threads N]
#
#   --cpus N     CPU quota of the probe container (`docker run --cpus`), default 1
#   --runs N     repetitions of every probe, default 3 (the report takes the median)
#   --threads N  SHERPA_TTS_NUM_THREADS and SHERPA_STT_NUM_THREADS inside the container, default 1
#
# Builds the probes (crates/vendor-sherpa-onnx/tests/voice_cost_probe.rs) once in release mode,
# then runs them per voice and for the STT model, each in its own `docker run --cpus N`, and calls
# scripts/perf/voice-cost-report.mjs. Raw output lands in `.test-logs/perf/raw/`, the report in
# `.test-logs/perf/voice-cost-<stamp>.{json,md}`; build output in `.test-logs/<stamp>-voice-cost-build.log`.
#
# Environment:
#   PROBE_MODELS_DIR       (required) directory holding the model bundles; mounted read-only
#   VOICE_COST_VOICES      space separated TTS bundle directory names (default: vits-piper-en_US-amy-low
#                          vits-piper-en_US-amy-medium vits-piper-en_US-lessac-low
#                          vits-piper-en_US-lessac-medium vits-piper-en_US-lessac-high vits-melo-tts-zh_en)
#   VOICE_COST_STT         streaming STT bundle directory name
#                          (default: sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06)
#   PROBE_TTS_SESSIONS, PROBE_LONG_REPS, PROBE_STT_STREAMS, PROBE_STT_LAG_SLO_MS
#                          forwarded to the probes (defaults live in the probe file header)
#   SHERPA_POOL_MAX_CONCURRENT_TTS
#                          forwarded too; when unset the TTS probe raises it to the largest session count
#   PROBE_CI_IMAGE         override the build/run image. Default: `nwr-voice-cost:local`, built once from
#                          scripts/perf/voice-cost.Dockerfile (rust:1.99-bookworm, cmake, build-essential)
#                          when it is missing; delete the image to rebuild. An override must be a Linux
#                          image with a Rust toolchain, cmake and build-essential, pullable or local.
#                          The image runs at the host architecture; do not emulate another one, the
#                          timings would be meaningless.
#
# The container is pinned with `--cpus` only: run on a quiet machine and keep the host's other
# containers idle. Docker Desktop shares its VM's cores, so give the VM at least N + 1.
# Files the build writes go to the docker volumes `nwr-voice-cost-target` and `nwr-voice-cost-cargo`.
#
# Exit codes: 0 ok, 1 a probe run failed, 2 usage, 3 models missing, 4 image or probe build failed,
# 5 PROBE_CI_IMAGE override cannot be pulled.
set -euo pipefail

CPUS=1
RUNS=3
THREADS=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cpus) CPUS="${2:?--cpus needs a value}"; shift 2 ;;
    --runs) RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --threads) THREADS="${2:?--threads needs a value}"; shift 2 ;;
    -h | --help) sed -n '2,39p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

: "${PROBE_MODELS_DIR:?set PROBE_MODELS_DIR to the directory holding the model bundles}"
MODELS_DIR="$(cd "$PROBE_MODELS_DIR" 2>/dev/null && pwd)" || {
  echo "PROBE_MODELS_DIR is not a directory: $PROBE_MODELS_DIR" >&2
  exit 3
}
IMAGE="${PROBE_CI_IMAGE:-nwr-voice-cost:local}"
read -r -a VOICES <<<"${VOICE_COST_VOICES:-vits-piper-en_US-amy-low vits-piper-en_US-amy-medium vits-piper-en_US-lessac-low vits-piper-en_US-lessac-medium vits-piper-en_US-lessac-high vits-melo-tts-zh_en}"
STT_MODEL="${VOICE_COST_STT:-sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06}"

missing=0
for model in "${VOICES[@]}" "$STT_MODEL"; do
  if [[ ! -d "$MODELS_DIR/$model" ]]; then
    echo "model missing: $MODELS_DIR/$model" >&2
    missing=1
  fi
done
[[ $missing -eq 0 ]] || exit 3

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  if [[ -z "${PROBE_CI_IMAGE:-}" ]]; then
    echo "==> building $IMAGE from scripts/perf/voice-cost.Dockerfile"
    docker build -q -f scripts/perf/voice-cost.Dockerfile -t "$IMAGE" scripts/perf >/dev/null || {
      echo "cannot build $IMAGE from scripts/perf/voice-cost.Dockerfile" >&2
      exit 4
    }
  else
    echo "==> pulling $IMAGE"
    docker pull "$IMAGE" >/dev/null 2>&1 || {
      echo "cannot pull $IMAGE; PROBE_CI_IMAGE must be a Linux image with Rust, cmake and build-essential" >&2
      exit 5
    }
  fi
fi

STAMP="$(date +%Y%m%d-%H%M%S)"
RAW_DIR=".test-logs/perf/raw"
mkdir -p "$RAW_DIR"
BUILD_LOG=".test-logs/${STAMP}-voice-cost-build.log"
TARGET_VOLUME="nwr-voice-cost-target"
CARGO_VOLUME="nwr-voice-cost-cargo"

echo "==> building probes in $IMAGE (log: $BUILD_LOG)"
if ! docker run --rm \
  -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
  -e CARGO_TARGET_DIR=/target \
  -v "$ROOT:/workspace" \
  -v "$TARGET_VOLUME:/target" \
  -v "$CARGO_VOLUME:/usr/local/cargo/registry" \
  -w /workspace \
  "$IMAGE" \
  bash -c 'cargo test --release -p node-webrtc-rust-vendor-sherpa-onnx --test voice_cost_probe --no-run \
    && bin="$(ls -t /target/release/deps/voice_cost_probe-* | grep -v "\.d$" | head -1)" \
    && cp "$bin" /target/voice_cost_probe' >"$BUILD_LOG" 2>&1; then
  echo "probe build failed, see $BUILD_LOG" >&2
  exit 4
fi

PROBE_ENV=()
for name in PROBE_TTS_SESSIONS PROBE_LONG_REPS PROBE_STT_STREAMS PROBE_STT_LAG_SLO_MS SHERPA_POOL_MAX_CONCURRENT_TTS; do
  if [[ -n "${!name:-}" ]]; then PROBE_ENV+=(-e "$name=${!name}"); fi
done

# run_probe <log> <probe test name> <env var holding the model path> <model dir name>
run_probe() {
  local log="$1" test_name="$2" model_var="$3" model="$4"
  docker run --rm --cpus "$CPUS" \
    -e "$model_var=/models/$model" \
    -e "SHERPA_TTS_NUM_THREADS=$THREADS" \
    -e "SHERPA_STT_NUM_THREADS=$THREADS" \
    ${PROBE_ENV[@]+"${PROBE_ENV[@]}"} \
    -v "$TARGET_VOLUME:/target:ro" \
    -v "$ROOT:/workspace:ro" \
    -v "$MODELS_DIR:/models:ro" \
    -w /workspace \
    "$IMAGE" \
    /target/voice_cost_probe "$test_name" --ignored --nocapture --test-threads=1 >"$log" 2>&1 || {
    echo "probe failed, see $log" >&2
    exit 1
  }
}

LOGS=()
for ((run = 1; run <= RUNS; run++)); do
  for voice in "${VOICES[@]}"; do
    echo "==> run $run/$RUNS tts $voice (--cpus $CPUS, $THREADS threads)"
    log="$RAW_DIR/${STAMP}-run${run}-tts-${voice}.log"
    run_probe "$log" tts_voice_cost_probe SHERPA_TTS_MODEL_PATH "$voice"
    LOGS+=("$log")
  done
  echo "==> run $run/$RUNS stt $STT_MODEL (--cpus $CPUS, $THREADS threads)"
  log="$RAW_DIR/${STAMP}-run${run}-stt-${STT_MODEL}.log"
  run_probe "$log" stt_stream_capacity_probe SHERPA_STT_MODEL_PATH "$STT_MODEL"
  LOGS+=("$log")
done

OUT=".test-logs/perf/voice-cost-${STAMP}"
node scripts/perf/voice-cost-report.mjs --out "$OUT.json" --md "$OUT.md" "${LOGS[@]}"
