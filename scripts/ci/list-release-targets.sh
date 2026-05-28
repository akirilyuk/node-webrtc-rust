#!/usr/bin/env bash
# Canonical release native targets (napi-rs triples). Used by plan/build/stage scripts.
set -euo pipefail

cat <<'EOF'
x86_64-unknown-linux-gnu
x86_64-unknown-linux-musl
aarch64-unknown-linux-gnu
x86_64-apple-darwin
aarch64-apple-darwin
x86_64-pc-windows-msvc
EOF
