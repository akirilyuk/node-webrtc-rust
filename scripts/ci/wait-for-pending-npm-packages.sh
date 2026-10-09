#!/usr/bin/env bash
# Wait (bounded) for every package recorded in NPM_PUBLISH_PENDING_FILE to become visible.
#
# The file is appended to by publish-npm-if-needed.sh when NPM_PUBLISH_DEFER_VERIFY=1 and the
# freshly published package was not yet visible. Format: one "<pkg> <version>" per line.
#
# All pending packages share one budget (NPM_REGISTRY_VERIFY_TOTAL_SECONDS, default 7200 = 2 h),
# polled round-robin with backoff (3s doubling, cap NPM_REGISTRY_VERIFY_MAX_SLEEP default 60).
# Exits 1 listing every package still missing when the budget is spent; exits 0 (and truncates
# the file) when all are visible or nothing is pending. SLEEP_BIN is for tests.
#
# Why 2 h: registry lag is unbounded in practice. 0.9.18 took ~17 min (33 MB linux-arm64);
# 0.9.34 (run 37715035913) took ~90 min for the 87 MB bindings-darwin-x64 tarball, which outran the
# old 30 min budget and left bindings/sdk/helpers/signaling unpublished. Keep the release.yml
# "Publish release" job timeout-minutes above this budget (guard: wait-for-pending-npm-packages.test.sh).
#
# Optional wall-clock deadline: when NPM_PUBLISH_DEADLINE_EPOCH (unix seconds) is set it replaces the
# TOTAL/elapsed check. Before each sleep remaining = deadline - now; at <= 0 polling stops ("deadline
# reached") and each sleep is min(delay, remaining). The clock is NPM_REGISTRY_VERIFY_NOW_BIN
# (default `date +%s`, invoked as a command; tests supply a fake). Probe time counts against the
# deadline, so one deadline can bound every registry wait in a job. Unset: behaviour is unchanged.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
FILE="${NPM_PUBLISH_PENDING_FILE:?NPM_PUBLISH_PENDING_FILE required}"
TOTAL="${NPM_REGISTRY_VERIFY_TOTAL_SECONDS:-7200}"
SLEEP_SECONDS="${NPM_REGISTRY_VERIFY_SLEEP_SECONDS:-3}"
MAX_SLEEP="${NPM_REGISTRY_VERIFY_MAX_SLEEP:-60}"
SLEEP_BIN="${NPM_REGISTRY_VERIFY_SLEEP_BIN:-sleep}"
NOW_CMD="${NPM_REGISTRY_VERIFY_NOW_BIN:-date +%s}"
DEADLINE="${NPM_PUBLISH_DEADLINE_EPOCH:-}"

if [[ ! -s "$FILE" ]]; then
  exit 0
fi

pending=()
while IFS= read -r line; do
  [[ -n "$line" ]] && pending+=("$line")
done <"$FILE"

echo "==> Waiting for ${#pending[@]} deferred package(s) to appear on registry (budget ${TOTAL}s)"
elapsed=0
delay="$SLEEP_SECONDS"
while :; do
  still=()
  for entry in "${pending[@]}"; do
    read -r pkg version <<<"$entry"
    if bash "$HERE/npm-registry-visible.sh" "$pkg" "$version"; then
      echo "  verified ${pkg}@${version} on registry (after ~${elapsed}s)"
    else
      still+=("$entry")
    fi
  done
  if [[ ${#still[@]} -eq 0 ]]; then
    : >"$FILE"
    exit 0
  fi
  pending=("${still[@]}")
  if [[ -n "$DEADLINE" ]]; then
    remaining=$((DEADLINE - $($NOW_CMD)))
    if [[ "$remaining" -le 0 ]]; then
      echo "deadline reached (NPM_PUBLISH_DEADLINE_EPOCH=${DEADLINE})" >&2
      break
    fi
    nap="$delay"
    if [[ "$remaining" -lt "$nap" ]]; then nap="$remaining"; fi
    echo "  ${#pending[@]} still missing, sleep ${nap}s (${remaining}s to deadline)..."
  else
    if [[ "$elapsed" -ge "$TOTAL" ]]; then
      break
    fi
    nap="$delay"
    echo "  ${#pending[@]} still missing, sleep ${delay}s (~${elapsed}s/${TOTAL}s)..."
  fi
  "$SLEEP_BIN" "$nap"
  elapsed=$((elapsed + nap))
  next=$((delay * 2))
  if [[ "$next" -gt "$MAX_SLEEP" ]]; then delay="$MAX_SLEEP"; else delay="$next"; fi
done

for entry in "${pending[@]}"; do
  echo "Not on npm registry after publish: ${entry/ /@} (after ${elapsed}s)" >&2
done
exit 1
