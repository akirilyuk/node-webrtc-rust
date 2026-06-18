#!/usr/bin/env bash
# Fingerprint inputs that change the native .node binary or committed NAPI surface.
# Used by CI native-binding-cache (exact key match only — no restore-key prefix fallback).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

{
  sha256sum Cargo.lock
  sha256sum \
    packages/bindings/Cargo.toml \
    packages/bindings/build.rs \
    packages/bindings/package.json \
    packages/bindings/index.d.ts \
    packages/bindings/index.js

  if [[ -d packages/bindings/src ]]; then
    find packages/bindings/src -type f | sort | xargs sha256sum
  fi

  for crate in core mixer conference; do
    sha256sum "crates/${crate}/Cargo.toml"
    if [[ -d "crates/${crate}/src" ]]; then
      find "crates/${crate}/src" -type f -name '*.rs' | sort | xargs sha256sum
    fi
  done

  # Musl prebuilds must rebuild when Alpine native toolchain changes (not Zig cross).
  sha256sum \
    docker/ci/Dockerfile.alpine \
    scripts/ci/install-alpine-native-toolchain.sh \
    scripts/ci/verify-musl-runtime.sh
} | sha256sum | awk '{print $1}'
