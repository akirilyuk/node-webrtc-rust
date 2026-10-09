#!/usr/bin/env bash
# Poll npm registry until pkg@version is visible after publish (eventual consistency).
#
# Visibility check: scripts/ci/npm-registry-visible.sh (`npm view` or registry GET /{pkg}/{version}).
#
# Sleep starts at NPM_REGISTRY_VERIFY_SLEEP_SECONDS (default 3) and doubles each
# wait, capped at NPM_REGISTRY_VERIFY_MAX_SLEEP (default 60). Defaults (35 attempts)
# give ~30 min of polling: large tarballs (e.g. 33 MB linux-arm64 binding) were
# observed to take ~17 min to become visible (release 0.9.18). Override via env.
# SLEEP_BIN is for tests (default sleep).
#
# Optional wall-clock deadline: when NPM_PUBLISH_DEADLINE_EPOCH (unix seconds) is set, MAX_ATTEMPTS is
# ignored and polling stops once the deadline passes ("deadline reached"). Each sleep is
# min(delay, deadline - now). The clock is NPM_REGISTRY_VERIFY_NOW_BIN (default `date +%s`, invoked
# as a command; tests supply a fake). Unset: behaviour is unchanged.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
PKG="${1:?package name required}"
VERSION="${2:?version required}"
MAX_ATTEMPTS="${NPM_REGISTRY_VERIFY_ATTEMPTS:-35}"
SLEEP_SECONDS="${NPM_REGISTRY_VERIFY_SLEEP_SECONDS:-3}"
MAX_SLEEP="${NPM_REGISTRY_VERIFY_MAX_SLEEP:-60}"
SLEEP_BIN="${NPM_REGISTRY_VERIFY_SLEEP_BIN:-sleep}"
NOW_CMD="${NPM_REGISTRY_VERIFY_NOW_BIN:-date +%s}"
DEADLINE="${NPM_PUBLISH_DEADLINE_EPOCH:-}"

delay="$SLEEP_SECONDS"
if [[ -n "$DEADLINE" ]]; then
  MAX_ATTEMPTS=999999999
fi
for ((attempt = 1; attempt <= MAX_ATTEMPTS; attempt++)); do
  if bash "$HERE/npm-registry-visible.sh" "$PKG" "$VERSION"; then
    echo "  verified ${PKG}@${VERSION} on registry (attempt ${attempt}/${MAX_ATTEMPTS})"
    exit 0
  fi
  if [[ -n "$DEADLINE" ]]; then
    remaining=$((DEADLINE - $($NOW_CMD)))
    if [[ "$remaining" -le 0 ]]; then
      echo "Not on npm registry after publish: ${PKG}@${VERSION} (deadline reached after ${attempt} attempts)" >&2
      exit 1
    fi
    nap="$delay"
    if [[ "$remaining" -lt "$nap" ]]; then nap="$remaining"; fi
    echo "  waiting for ${PKG}@${VERSION} on registry (attempt ${attempt}, sleep ${nap}s, ${remaining}s to deadline)..."
    "$SLEEP_BIN" "$nap"
    next=$((delay * 2))
    if [[ "$next" -gt "$MAX_SLEEP" ]]; then
      delay="$MAX_SLEEP"
    else
      delay="$next"
    fi
  elif [[ "$attempt" -lt "$MAX_ATTEMPTS" ]]; then
    echo "  waiting for ${PKG}@${VERSION} on registry (attempt ${attempt}/${MAX_ATTEMPTS}, sleep ${delay}s)..."
    "$SLEEP_BIN" "$delay"
    next=$((delay * 2))
    if [[ "$next" -gt "$MAX_SLEEP" ]]; then
      delay="$MAX_SLEEP"
    else
      delay="$next"
    fi
  fi
done

echo "Not on npm registry after publish: ${PKG}@${VERSION} (after ${MAX_ATTEMPTS} attempts)" >&2
exit 1
