#!/usr/bin/env bash
# Resolve reusable native-main-bundle from successful Build & Test (main) runs.
# Writes GITHUB_OUTPUT; exit 0 even on fallback (bundle_reused=false).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

profile="${NATIVE_BUNDLE_PROFILE:-release}"
download_dir="${NATIVE_BUNDLE_DOWNLOAD_DIR:-${RUNNER_TEMP:-/tmp}/native-main-bundle}"
extra=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile) profile="${2:-}"; shift 2 ;;
    --download-dir) download_dir="${2:-}"; shift 2 ;;
    --fixture-dir) extra+=(--fixture-dir "${2:-}"); shift 2 ;;
    --sha) extra+=(--sha "${2:-}"); shift 2 ;;
    *)
      echo "resolve-native-main-bundle: unknown arg $1" >&2
      exit 1
      ;;
  esac
done

export NATIVE_TOOL_MODE="${NATIVE_TOOL_MODE:-declared}"
mkdir -p "$download_dir"
exec python3 scripts/ci/resolve_native_main_bundle.py \
  --profile "$profile" \
  --download-dir "$download_dir" \
  "${extra[@]}"
