#!/usr/bin/env bash
# Fingerprint inputs that change the native .node binary or committed NAPI surface.
# Used by CI native-binding-cache (exact key match only — no restore-key prefix fallback).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

hash_crate_sources() {
  local crate_dir="$1"
  sha256sum "${crate_dir}/Cargo.toml"
  if [[ -d "${crate_dir}/src" ]]; then
    find "${crate_dir}/src" -type f -name '*.rs' | sort | xargs sha256sum
  fi
  if [[ -f "${crate_dir}/build.rs" ]]; then
    sha256sum "${crate_dir}/build.rs"
  fi
}

{
  sha256sum Cargo.toml Cargo.lock
  sha256sum \
    packages/bindings/Cargo.toml \
    packages/bindings/build.rs \
    packages/bindings/package.json \
    packages/bindings/index.d.ts \
    packages/bindings/index.js

  if [[ -d packages/bindings/src ]]; then
    find packages/bindings/src -type f | sort | xargs sha256sum
  fi

  # Every crate linked into packages/bindings (see bindings/Cargo.toml). signaling is
  # workspace-only and not part of the .node — exclude it to avoid spurious rebuilds.
  for crate_dir in crates/*/; do
    crate="$(basename "$crate_dir")"
    [[ "$crate" == signaling ]] && continue
    hash_crate_sources "$crate_dir"
  done

  # Musl prebuilds must rebuild when Alpine native toolchain changes (not Zig cross).
  sha256sum \
    docker/ci/Dockerfile.alpine \
    scripts/ci/install-alpine-native-toolchain.sh \
    scripts/ci/build-sherpa-onnx-musl-libs.sh \
    scripts/ci/verify-musl-runtime.sh
} | sha256sum | awk '{print $1}'
