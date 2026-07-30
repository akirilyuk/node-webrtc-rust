#!/usr/bin/env bash
# Target-specific native input fingerprint (canonical contract).
#
# Usage:
#   bash scripts/ci/native-build-fingerprint.sh --target TRIPLE [--profile debug|release]
#   bash scripts/ci/native-build-fingerprint.sh --distribution --target TRIPLE
#   bash scripts/ci/native-build-fingerprint.sh --list-local-crates --target TRIPLE
#
# Optional CI tool-identity env (chunk 2 fills these from the builder host/image):
#   NATIVE_RUSTC_IDENTITY NATIVE_CARGO_IDENTITY NATIVE_NODE_IDENTITY
#   NATIVE_IMAGE_DIGEST NATIVE_RUNNER_LABEL NATIVE_HOST_SDK_IDENTITY
#   NATIVE_ZIG_IDENTITY NATIVE_NAPI_CLI_IDENTITY NATIVE_CACHE_EPOCH
#   NATIVE_SHERPA_ONNX_LIB_DIR
#
# Until workflows pin toolchains, unresolved tool slots stay the literal string
# "unresolved" so local digests remain deterministic.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PY="$ROOT/scripts/ci/native_build_contract.py"

mode="fingerprint"
target=""
profile="release"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      target="${2:-}"
      shift 2
      ;;
    --profile)
      profile="${2:-}"
      shift 2
      ;;
    --distribution)
      mode="distribution"
      shift
      ;;
    --list-local-crates)
      mode="list-local-crates"
      shift
      ;;
    --contract-json)
      mode="contract-json"
      shift
      ;;
    -h | --help)
      sed -n '2,18p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "native-build-fingerprint: unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$target" ]]; then
  echo "native-build-fingerprint: --target is required" >&2
  exit 1
fi

case "$mode" in
  fingerprint)
    exec python3 "$PY" fingerprint --target "$target" --profile "$profile"
    ;;
  distribution)
    exec python3 "$PY" distribution-digest --target "$target"
    ;;
  list-local-crates)
    exec python3 "$PY" list-local-crates --target "$target" --profile "$profile"
    ;;
  contract-json)
    exec python3 "$PY" contract-json --target "$target" --profile "$profile" --with-digests
    ;;
  *)
    echo "native-build-fingerprint: internal error mode=$mode" >&2
    exit 1
    ;;
esac
