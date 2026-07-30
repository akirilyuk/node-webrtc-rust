#!/usr/bin/env bash
# Canonical compile recipe for native .node artifacts.
#
# This file is part of the native input fingerprint. Keep cache, manifest,
# artifact, and workflow orchestration outside this script so those changes can
# reuse already-compiled Rust binaries.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BINDINGS_DIR="${NATIVE_BUILD_BINDINGS_DIR:-$ROOT/packages/bindings}"
target=""
profile="release"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) target="${2:-}"; shift 2 ;;
    --profile) profile="${2:-}"; shift 2 ;;
    *)
      echo "build-native-addon: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$target" ]]; then
  echo "build-native-addon: --target is required" >&2
  exit 2
fi
if [[ "$profile" != "debug" && "$profile" != "release" ]]; then
  echo "build-native-addon: --profile must be debug or release" >&2
  exit 2
fi

case "$target" in
  x86_64-unknown-linux-gnu | x86_64-unknown-linux-musl | aarch64-unknown-linux-gnu | x86_64-apple-darwin | aarch64-apple-darwin | x86_64-pc-windows-msvc) ;;
  *)
    echo "build-native-addon: unsupported release target: $target" >&2
    exit 2
    ;;
esac

# These values are part of native_build_contract.py and must not inherit
# untracked runner state.
export CMAKE_POLICY_VERSION_MINIMUM=3.5
export OPUS_STATIC=1
unset SHERPA_ONNX_LIB_DIR
if [[ "$target" == "x86_64-unknown-linux-musl" ]]; then
  export SHERPA_ONNX_LIB_DIR=/opt/sherpa-musl/lib
  export LD_LIBRARY_PATH="${SHERPA_ONNX_LIB_DIR}:/usr/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
fi

cd "$BINDINGS_DIR"
rm -f ./*.node

args=(build --target "$target")
if [[ "$profile" == "release" ]]; then
  args+=(--release --features otel)
  args+=(--platform)
fi

npx napi "${args[@]}"
if [[ "$profile" == "debug" ]]; then
  npm run copy:local-node
fi
