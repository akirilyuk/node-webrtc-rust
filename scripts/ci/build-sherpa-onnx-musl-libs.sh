#!/usr/bin/env bash
# Build Sherpa-ONNX shared libs on Alpine/musl against the distro onnxruntime package.
#
# sherpa-onnx-sys (static, default) downloads glibc prebuilt archives — dlopen on Alpine
# fails with "__strdup: symbol not found". Musl CI/release must set SHERPA_ONNX_LIB_DIR to
# musl-built shared libs and compile bindings for target x86_64-unknown-linux-musl (Cargo selects link-shared via target_env).
#
# Usage: bash scripts/ci/build-sherpa-onnx-musl-libs.sh
#
# Env:
#   SHERPA_MUSL_PREFIX  install root (default: /opt/sherpa-musl)
#   SHERPA_ONNX_VERSION tag without v (default: 1.13.2)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SHERPA_VERSION="${SHERPA_ONNX_VERSION:-1.13.2}"
PREFIX="${SHERPA_MUSL_PREFIX:-/opt/sherpa-musl}"
MARKER="$PREFIX/.sherpa-musl-v${SHERPA_VERSION}"

if [[ -f "$MARKER" ]]; then
  echo "==> Sherpa musl libs already built ($MARKER)"
  echo "SHERPA_ONNX_LIB_DIR=$PREFIX/lib"
  exit 0
fi

if ! grep -qi alpine /etc/os-release 2>/dev/null; then
  echo "build-sherpa-onnx-musl-libs.sh must run on Alpine Linux" >&2
  exit 1
fi

echo "==> Installing Sherpa build deps (Alpine musl onnxruntime)"
apk add --no-cache \
  build-base \
  cmake \
  curl \
  git \
  linux-headers \
  onnxruntime-dev \
  patch \
  pkgconfig \
  python3 \
  bash

mkdir -p "$PREFIX/src"
SRC="$PREFIX/src/sherpa-onnx"
if [[ ! -d "$SRC/.git" ]]; then
  rm -rf "$SRC"
  git clone --depth 1 --branch "v${SHERPA_VERSION}" \
    https://github.com/k2-fsa/sherpa-onnx.git "$SRC"
fi

BUILD="$PREFIX/build"
rm -rf "$BUILD"
mkdir -p "$BUILD"

export SHERPA_ONNXRUNTIME_INCLUDE_DIR="${SHERPA_ONNXRUNTIME_INCLUDE_DIR:-/usr/include/onnxruntime}"
export SHERPA_ONNXRUNTIME_LIB_DIR="${SHERPA_ONNXRUNTIME_LIB_DIR:-/usr/lib}"

echo "==> Configuring Sherpa-ONNX v${SHERPA_VERSION} (shared, system musl onnxruntime)"
cmake -S "$SRC" -B "$BUILD" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DCMAKE_CXX_FLAGS="-include cstdint -I${SHERPA_ONNXRUNTIME_INCLUDE_DIR}" \
  -DBUILD_SHARED_LIBS=ON \
  -DSHERPA_ONNX_ENABLE_C_API=ON \
  -DSHERPA_ONNX_ENABLE_BINARY=OFF \
  -DSHERPA_ONNX_ENABLE_TESTS=OFF \
  -DSHERPA_ONNX_ENABLE_PYTHON=OFF \
  -DSHERPA_ONNX_ENABLE_PORTAUDIO=OFF \
  -DSHERPA_ONNX_USE_PRE_INSTALLED_ONNXRUNTIME_IF_AVAILABLE=ON

echo "==> Building Sherpa-ONNX"
cmake --build "$BUILD" -j"$(nproc)"
cmake --install "$BUILD"

if [[ ! -f "$PREFIX/lib/libsherpa-onnx-c-api.so" ]] \
  && [[ ! -f "$PREFIX/lib/libsherpa-onnx-c-api.so.1" ]]; then
  echo "ERROR: missing libsherpa-onnx-c-api.so under $PREFIX/lib" >&2
  ls -la "$PREFIX/lib" >&2 || true
  exit 1
fi

touch "$MARKER"
echo "==> Sherpa musl libs installed to $PREFIX/lib"
echo "    Set SHERPA_ONNX_LIB_DIR=$PREFIX/lib and LD_LIBRARY_PATH=$PREFIX/lib:/usr/lib"
echo "    Build bindings with: napi build --platform --target x86_64-unknown-linux-musl"
