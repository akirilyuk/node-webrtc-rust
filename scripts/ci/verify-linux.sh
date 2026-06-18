#!/usr/bin/env bash
# Mirror GitHub Actions Linux native builds locally (CI Docker image + napi matrix).
# Usage:
#   ./scripts/ci/verify-linux.sh              # all Linux CI targets
#   ./scripts/ci/verify-linux.sh x86_64-unknown-linux-gnu
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${CI_IMAGE_LOCAL:-node-webrtc-rust-ci-build:local}"
ALPINE_IMAGE="${CI_ALPINE_IMAGE_LOCAL:-node-webrtc-rust-ci-alpine:local}"
TARGETS=(
  x86_64-unknown-linux-gnu
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-gnu
)

if [[ $# -gt 0 ]]; then
  TARGETS=("$@")
fi

echo "==> Building CI Docker image: $IMAGE"
docker build -t "$IMAGE" "$ROOT/docker/ci"

echo "==> Building Alpine CI Docker image: $ALPINE_IMAGE"
docker build -t "$ALPINE_IMAGE" -f "$ROOT/docker/ci/Dockerfile.alpine" "$ROOT"

echo "==> Installing bindings dependencies (Ubuntu CI image)"
docker run --rm -v "$ROOT:/workspace" -w /workspace/packages/bindings "$IMAGE" \
  npm ci --ignore-scripts

for target in "${TARGETS[@]}"; do
  if [[ "$target" == "x86_64-unknown-linux-musl" ]]; then
    echo "==> napi build --platform --release --target $target (native Alpine, no --zig)"
    docker run --rm \
      -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
      -e OPUS_STATIC=1 \
      -v "$ROOT:/workspace" \
      -w /workspace/packages/bindings \
      "$ALPINE_IMAGE" \
      npx napi build --platform --release --target "$target"
    bash "$ROOT/scripts/ci/verify-musl-runtime.sh"
    continue
  fi

  zig_flag=(--zig)
  if [[ "$target" == "x86_64-unknown-linux-gnu" ]]; then
    echo "==> napi build --platform --release --target $target (native gnu, no --zig)"
    docker run --rm \
      -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
      -e OPUS_STATIC=1 \
      -v "$ROOT:/workspace" \
      -w /workspace/packages/bindings \
      "$IMAGE" \
      npx napi build --platform --release --target "$target"
    continue
  fi

  echo "==> napi build --platform --release --target $target ${zig_flag[*]}"
  docker run --rm \
    -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
    -e OPUS_STATIC=1 \
    -v "$ROOT:/workspace" \
    -w /workspace/packages/bindings \
    "$IMAGE" \
    npx napi build --platform --release --target "$target" "${zig_flag[@]}"
done

echo "==> Linux CI native builds OK"
