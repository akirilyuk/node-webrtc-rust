#!/usr/bin/env bash
# Run a command with a wall-clock cap (GNU timeout / gtimeout when available).
#
# Usage:
#   bash scripts/ci/run-with-timeout.sh <seconds> <label> -- <command...>
#
# Env:
#   CI_STEP_TIMEOUT_DISABLE=1   skip the cap (debug only)
#
# Exit 124 when the cap is hit (GNU timeout convention).
set -euo pipefail

if [[ $# -lt 4 ]] || [[ "${3:-}" != "--" ]]; then
  echo "Usage: $0 <seconds> <label> -- <command...>" >&2
  exit 2
fi

SECS="$1"
LABEL="$2"
shift 3

if [[ "${CI_STEP_TIMEOUT_DISABLE:-}" == "1" ]]; then
  echo "==> [$LABEL] (timeout disabled)"
  exec "$@"
fi

if ! [[ "$SECS" =~ ^[0-9]+$ ]] || [[ "$SECS" -lt 1 ]]; then
  echo "Invalid timeout seconds: $SECS" >&2
  exit 2
fi

resolve_timeout_cmd() {
  if command -v timeout >/dev/null 2>&1; then
    echo timeout
  elif command -v gtimeout >/dev/null 2>&1; then
    echo gtimeout
  fi
}

TIMEOUT_CMD="$(resolve_timeout_cmd)"

echo "==> [$LABEL] (max ${SECS}s)"

if [[ -n "$TIMEOUT_CMD" ]]; then
  set +e
  "$TIMEOUT_CMD" --foreground "${SECS}s" "$@"
  code=$?
  set -e
  if [[ $code -ne 0 ]]; then
    if [[ $code -eq 124 ]]; then
      echo "==> [$LABEL] TIMED OUT after ${SECS}s" >&2
    fi
    exit "$code"
  fi
  exit 0
fi

echo "WARN: no timeout/gtimeout — running [$LABEL] without a wall-clock cap" >&2
exec "$@"
