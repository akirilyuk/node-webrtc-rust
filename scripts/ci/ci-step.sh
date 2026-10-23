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
#   CI_STEP_CPU_STAT_FILE         — cgroup v2 cpu.stat to read (default /sys/fs/cgroup/cpu.stat). When
#                                   readable, each step prints one extra line after OK/FAIL:
#                                   "[ci-step]       cpu: <s>s over <wall>s (avg <n> cores) <label>".
#                                   Not readable (macOS): nothing is printed.
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

# cgroup CPU time (usage_usec of the whole job pod, so parallel work outside this step counts too).
CPU_STAT_FILE="${CI_STEP_CPU_STAT_FILE:-/sys/fs/cgroup/cpu.stat}"
cpu_usage_usec() {
  [[ -r "$CPU_STAT_FILE" ]] || return 0
  awk '$1 == "usage_usec" { print $2; exit }' "$CPU_STAT_FILE" 2>/dev/null || true
}

# Prints the cpu line to stdout (arg "out") or stderr (arg "err"). Needs CPU_START_USEC, ELAPSED, LABEL.
ci_step_cpu_line() {
  local stream="$1" cpu_end line
  [[ -n "$CPU_START_USEC" ]] || return 0
  cpu_end="$(cpu_usage_usec)"
  [[ -n "$cpu_end" ]] || return 0
  line="$(awk -v s="$CPU_START_USEC" -v e="$cpu_end" -v w="$ELAPSED" -v l="$LABEL" 'BEGIN {
    cpu = (e - s) / 1000000
    if (w + 0 > 0) avg = sprintf("%.2f cores", cpu / w); else avg = "n/a"
    printf "[ci-step]       cpu: %.1fs over %ss (avg %s) %s", cpu, w, avg, l
  }')" || return 0
  if [[ "$stream" == "err" ]]; then echo "$line" >&2; else echo "$line"; fi
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

CPU_START_USEC="$(cpu_usage_usec)"
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
  ci_step_cpu_line out
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
  ci_step_cpu_line err
  if [[ $CODE -eq 124 ]]; then
    echo "[ci-step]       hint: step hit wall-clock cap — see label above or raise CI_*_TIMEOUT_SEC" >&2
  fi
fi

exit "$CODE"
