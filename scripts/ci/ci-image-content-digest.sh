#!/usr/bin/env bash
# Stable content digest for CI Docker image contracts (not mutable :latest).
# Usage: bash scripts/ci/ci-image-content-digest.sh glibc|alpine
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

KIND="${1:-}"
case "$KIND" in
  glibc | ci-build)
    PATHS=(
      docker/ci/Dockerfile
    )
    ;;
  alpine | ci-build-alpine)
    PATHS=(
      docker/ci/Dockerfile.alpine
      scripts/ci/install-alpine-native-toolchain.sh
      scripts/ci/build-sherpa-onnx-musl-libs.sh
    )
    ;;
  *)
    echo "usage: $0 glibc|alpine" >&2
    exit 2
    ;;
esac

{
  for p in "${PATHS[@]}"; do
    [[ -f "$p" ]] || {
      echo "missing: $p" >&2
      exit 1
    }
    printf '%s\0' "$p"
    cat "$p"
  done
} | shasum -a 256 | awk '{print $1}'
