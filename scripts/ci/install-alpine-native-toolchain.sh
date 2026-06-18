#!/usr/bin/env bash
# Native musl toolchain for Alpine CI jobs and local verify-linux musl builds.
set -euo pipefail

if ! grep -qi alpine /etc/os-release 2>/dev/null; then
  echo "install-alpine-native-toolchain.sh must run on Alpine Linux" >&2
  exit 1
fi

apk add --no-cache \
  bash \
  build-base \
  ca-certificates \
  cmake \
  curl \
  git \
  linux-headers \
  openssl-dev \
  pkgconfig \
  python3

export RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
export CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}"
export PATH="${CARGO_HOME}/bin:${PATH}"

if ! command -v rustc >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile minimal
fi

rustup target add x86_64-unknown-linux-musl

echo "Alpine native toolchain ready: $(rustc --version)"
