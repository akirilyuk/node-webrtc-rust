#!/usr/bin/env bash
# Assemble / validate / stage the six-target native main bundle.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PY="$ROOT/scripts/ci/native_build_contract.py"

if [[ $# -lt 1 ]]; then
  cat <<'EOF'
Usage:
  native-artifact-bundle.sh assemble --artifacts-root DIR --output DIR [--profile release]
  native-artifact-bundle.sh validate --bundle DIR [--profile release]
  native-artifact-bundle.sh stage --bundle DIR --output DIR
  native-artifact-bundle.sh aggregate [--profile release]
EOF
  exit 1
fi

cmd="$1"
shift
case "$cmd" in
  assemble)
    exec python3 "$PY" assemble-bundle "$@"
    ;;
  validate)
    exec python3 "$PY" validate-bundle "$@"
    ;;
  stage)
    exec python3 "$PY" stage-bundle "$@"
    ;;
  aggregate)
    profile=release
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --profile) profile="${2:-}"; shift 2 ;;
        *) echo "unknown: $1" >&2; exit 1 ;;
      esac
    done
    exec python3 "$PY" aggregate-digest --profile "$profile"
    ;;
  *)
    echo "unknown command: $cmd" >&2
    exit 1
    ;;
esac
