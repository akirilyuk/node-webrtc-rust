#!/usr/bin/env bash
# Build x86_64-unknown-linux-musl bindings inside the Alpine CI image and verify dlopen.
#
# Usage (from repo root):
#   bash scripts/ci/verify-musl-native-build-docker.sh
#
# Env:
#   CI_ALPINE_IMAGE  Docker image (default: local tag ci-build-alpine:verify)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${CI_ALPINE_IMAGE:-ci-build-alpine:verify}"
# CI musl job is x86_64 self-hosted; force amd64 so Apple Silicon hosts match CI.
DOCKER_PLATFORM="${DOCKER_PLATFORM:-linux/amd64}"
STAMP=$(date +%Y%m%d-%H%M%S)
LOG="$ROOT/.test-logs/${STAMP}-verify-musl-native-build-docker.log"
mkdir -p "$ROOT/.test-logs"

echo "==> Building Alpine CI image: $IMAGE (platform=$DOCKER_PLATFORM)"
docker build --platform "$DOCKER_PLATFORM" -t "$IMAGE" -f "$ROOT/docker/ci/Dockerfile.alpine" "$ROOT" >>"$LOG" 2>&1

echo "==> Musl native build + verify in container (log: $LOG)"
docker run --rm --platform "$DOCKER_PLATFORM" \
  -v "$ROOT:/workspace" \
  -w /workspace \
  "$IMAGE" \
  bash -lc '
    set -euo pipefail
    export SHERPA_MUSL_PREFIX=/opt/sherpa-musl
    bash scripts/ci/build-sherpa-onnx-musl-libs.sh
    export SHERPA_ONNX_LIB_DIR=/opt/sherpa-musl/lib
    export LD_LIBRARY_PATH=/opt/sherpa-musl/lib:/usr/lib
    export CMAKE_POLICY_VERSION_MINIMUM=3.5
    export OPUS_STATIC=1
    cd packages/bindings
    rm -rf node_modules
    rm -f *.node 2>/dev/null || true
    npm install --ignore-scripts --no-save --omit=optional @napi-rs/cli@^2.18.0
    npx napi build --platform --release \
      --target x86_64-unknown-linux-musl
    node ../../scripts/ci/verify-native-binding-surface.mjs --target x86_64-unknown-linux-musl
    bash ../../scripts/ci/verify-musl-runtime.sh
    echo "==> musl native build OK"
  ' >>"$LOG" 2>&1

echo "exit=0 log=$LOG"
