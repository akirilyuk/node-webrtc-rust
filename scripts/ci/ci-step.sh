#!/usr/bin/env bash
# Structured CI step logging — which step started, how long it took, pass/fail.
#
# Usage:
#   bash scripts/ci/ci-step.sh [--timeout SECONDS] <label> -- <command...>
#
# Env:
#   CI_STEP_INDEX / CI_STEP_TOTAL — "(3/12)" in banners
#   CI_STEP_LOG_TS=1              — UTC ISO timestamp on each line
#   CI_STEP_TIMEOUT_DISABLE=1     — skip wall-clock cap (debug)
#   CI_STEP_WARN_PCT              — warn when a passing step used at least this share of its
#                                   --timeout (default 75). Prints "[ci-step] WARN ...", a GitHub
#                                   ::warning annotation (GITHUB_ACTIONS) and a $GITHUB_STEP_SUMMARY
#                                   line, so a suite creeping toward its cap shows up before it
#                                   times out (test.time-budget-creep).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUN_WITH_TIMEOUT="$ROOT/scripts/ci/run-with-timeout.sh"

ci_step_prefix() {
  local idx=""
  if [[ -n "${CI_STEP_INDEX:-}" && -n "${CI_STEP_TOTAL:-}" ]]; then
    idx=" (${CI_STEP_INDEX}/${CI_STEP_TOTAL})"
  fi
  local ts=""
  if [[ "${CI_STEP_LOG_TS:-}" == "1" ]]; then
    ts="[$(date -u +%Y-%m-%dT%H:%M:%SZ)] "
  fi
  printf '%s[ci-step]%s' "$ts" "$idx"
}

SECS=""
LABEL=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --timeout)
      SECS="$2"
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      if [[ -z "$LABEL" ]]; then
        LABEL="$1"
        shift
      else
        echo "Unexpected argument: $1" >&2
        exit 2
      fi
      ;;
  esac
done

if [[ -z "$LABEL" ]] || [[ $# -eq 0 ]]; then
  echo "Usage: $0 [--timeout SEC] <label> -- <command...>" >&2
  exit 2
fi

START_SEC=$(date +%s)
echo "$(ci_step_prefix) START $LABEL"
echo "[ci-step]       cmd: $*"

set +e
if [[ -n "$SECS" ]]; then
  bash "$RUN_WITH_TIMEOUT" "$SECS" "$LABEL" -- "$@"
else
  "$@"
fi
CODE=$?
set -e

ELAPSED=$(( $(date +%s) - START_SEC ))

if [[ $CODE -eq 0 ]]; then
  echo "$(ci_step_prefix) OK    $LABEL (${ELAPSED}s)"
  # Early warning: ELAPSED is whole seconds (floor), so compare with >= to stay deterministic.
  WARN_PCT="${CI_STEP_WARN_PCT:-75}"
  if [[ -n "$SECS" && "$SECS" =~ ^[0-9]+$ && "$SECS" -ge 1 ]] \
    && [[ "${CI_STEP_TIMEOUT_DISABLE:-}" != "1" ]] \
    && (( ELAPSED * 100 >= WARN_PCT * SECS )); then
    USED_PCT=$(( ELAPSED * 100 / SECS ))
    echo "[ci-step] WARN $LABEL used ${ELAPSED}s of ${SECS}s (${USED_PCT}%)"
    if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
      echo "::warning title=CI step near its time budget::$LABEL ${ELAPSED}s/${SECS}s"
    fi
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
      echo "- CI step near its time budget: \`$LABEL\` used ${ELAPSED}s of ${SECS}s (${USED_PCT}%)" >>"$GITHUB_STEP_SUMMARY" || true
    fi
  fi
else
  echo "$(ci_step_prefix) FAIL  $LABEL (${ELAPSED}s, exit ${CODE})" >&2
  if [[ $CODE -eq 124 ]]; then
    echo "[ci-step]       hint: step hit wall-clock cap — see label above or raise CI_*_TIMEOUT_SEC" >&2
  fi
fi

exit "$CODE"
