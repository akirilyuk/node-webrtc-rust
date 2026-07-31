#!/usr/bin/env bash
# Produce + validate a per-target provenance manifest next to the .node.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

target=""
profile="release"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) target="${2:-}"; shift 2 ;;
    --profile) profile="${2:-}"; shift 2 ;;
    *)
      echo "write-native-artifact-manifest: unknown arg $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$target" ]]; then
  echo "write-native-artifact-manifest: --target required" >&2
  exit 1
fi

export NATIVE_TOOL_MODE=declared
eval "$(bash scripts/ci/collect-native-tool-identity.sh --target "$target")"

# If a pre-build snapshot exists (compile jobs), restore published surface bytes.
# Cache-hit / stage paths have no snapshot and never ran napi — leave checkout as-is.
if [[ -d "${NAPI_SURFACE_SNAPSHOT_DIR:-$ROOT/.napi-surface-snapshot}" ]]; then
  bash scripts/ci/bindings-napi-surface.sh restore
fi

mkdir -p packages/bindings/native-manifests
out="packages/bindings/native-manifests/${target}.json"
bash scripts/ci/native-artifact-manifest.sh produce \
  --target "$target" \
  --profile "$profile" \
  --output "$out"
bash scripts/ci/native-artifact-manifest.sh validate --manifest "$out"
echo "Wrote and validated $out"
