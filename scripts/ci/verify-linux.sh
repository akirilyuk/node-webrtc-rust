#!/usr/bin/env bash
# Mirror GitHub Actions Linux native builds locally (CI Docker image + napi matrix).
# Usage:
#   ./scripts/ci/verify-linux.sh              # all Linux CI targets
#   ./scripts/ci/verify-linux.sh x86_64-unknown-linux-gnu
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${CI_IMAGE_LOCAL:-node-webrtc-rust-ci-build:local}"
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

echo "==> Installing bindings dependencies"
docker run --rm -v "$ROOT:/workspace" -w /workspace/packages/bindings "$IMAGE" \
  npm ci --ignore-scripts

for target in "${TARGETS[@]}"; do
  echo "==> napi build --platform --release --target $target --zig"
  docker run --rm \
    -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
    -e OPUS_STATIC=1 \
    -v "$ROOT:/workspace" \
    -w /workspace/packages/bindings \
    "$IMAGE" \
    npx napi build --platform --release --target "$target" --zig
done

echo "==> Linux CI native builds OK"
