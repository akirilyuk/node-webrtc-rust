#!/usr/bin/env bash
# Produce / validate per-target native artifact provenance manifests.
#
# Produce:
#   bash scripts/ci/native-artifact-manifest.sh produce \
#     --target TRIPLE --profile release --output path/to/manifest.json [--node path]
#
# Validate (fail-closed on schema / checksum / contract mismatch):
#   bash scripts/ci/native-artifact-manifest.sh validate --manifest path/to/manifest.json
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PY="$ROOT/scripts/ci/native_build_contract.py"

if [[ $# -lt 1 ]]; then
  sed -n '2,12p' "$0" | sed 's/^# \?//'
  exit 1
fi

cmd="$1"
shift

case "$cmd" in
  produce)
    target=""
    profile="release"
    output=""
    node=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --target) target="${2:-}"; shift 2 ;;
        --profile) profile="${2:-}"; shift 2 ;;
        --output) output="${2:-}"; shift 2 ;;
        --node) node="${2:-}"; shift 2 ;;
        *)
          echo "native-artifact-manifest produce: unknown argument: $1" >&2
          exit 1
          ;;
      esac
    done
    if [[ -z "$target" || -z "$output" ]]; then
      echo "native-artifact-manifest produce: --target and --output are required" >&2
      exit 1
    fi
    args=(produce-manifest --target "$target" --profile "$profile" --output "$output")
    if [[ -n "$node" ]]; then
      args+=(--node "$node")
    fi
    exec python3 "$PY" "${args[@]}"
    ;;
  validate)
    manifest=""
    skip_recompute=0
    allow_missing_node=0
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --manifest) manifest="${2:-}"; shift 2 ;;
        --skip-recompute) skip_recompute=1; shift ;;
        --allow-missing-node) allow_missing_node=1; shift ;;
        *)
          echo "native-artifact-manifest validate: unknown argument: $1" >&2
          exit 1
          ;;
      esac
    done
    if [[ -z "$manifest" ]]; then
      echo "native-artifact-manifest validate: --manifest is required" >&2
      exit 1
    fi
    args=(validate-manifest --manifest "$manifest")
    if [[ "$skip_recompute" -eq 1 ]]; then
      args+=(--skip-recompute)
    fi
    if [[ "$allow_missing_node" -eq 1 ]]; then
      args+=(--allow-missing-node)
    fi
    exec python3 "$PY" "${args[@]}"
    ;;
  *)
    echo "native-artifact-manifest: unknown command: $cmd (use produce|validate)" >&2
    exit 1
    ;;
esac
