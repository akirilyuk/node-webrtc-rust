#!/usr/bin/env bash
# Rust format + lint gate on the toolchain pinned in rust-toolchain.toml.
# Vendored third_party/dtls and third_party/webrtc-ice are not workspace members and are not linted here.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "==> rustc $(rustc --version)"
echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check
echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings
echo "==> Rust lint OK"
