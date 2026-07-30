#!/usr/bin/env bash
# Native binding cache key — thin wrapper over native-v3 cache-key contract.
#
# Requires --target (or NATIVE_TARGET). Active CI callers always pass a target.
#
#   bash scripts/ci/native-binding-cache-key.sh --target TRIPLE [--profile release]
#   NATIVE_TARGET=… NATIVE_PROFILE=release bash scripts/ci/native-binding-cache-key.sh
#
# Optional: --aggregate prints the six-target aggregate digest (not a cache key).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

target="${NATIVE_TARGET:-}"
profile="${NATIVE_PROFILE:-release}"
aggregate=0

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
    --aggregate)
      aggregate=1
      shift
      ;;
    -h | --help)
      sed -n '2,14p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "native-binding-cache-key: unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

export NATIVE_TOOL_MODE="${NATIVE_TOOL_MODE:-declared}"

if [[ "$aggregate" -eq 1 ]]; then
  exec python3 scripts/ci/native_build_contract.py aggregate-digest --profile "$profile"
fi

if [[ -z "$target" ]]; then
  echo "native-binding-cache-key: --target (or NATIVE_TARGET) is required" >&2
  exit 1
fi

eval "$(bash scripts/ci/collect-native-tool-identity.sh --target "$target")"
# Print input digest only (Actions key prefix is added by native-binding-cache).
exec python3 scripts/ci/native_build_contract.py fingerprint --target "$target" --profile "$profile"
