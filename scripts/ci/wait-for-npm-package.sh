#!/usr/bin/env bash
# Poll npm registry until pkg@version is visible after publish (eventual consistency).
#
# Sleep starts at NPM_REGISTRY_VERIFY_SLEEP_SECONDS (default 3) and doubles each
# wait, capped at NPM_REGISTRY_VERIFY_MAX_SLEEP (default 30). Defaults give ~8 min
# of polling (20 attempts). Override via env. SLEEP_BIN is for tests (default sleep).
set -euo pipefail

PKG="${1:?package name required}"
VERSION="${2:?version required}"
MAX_ATTEMPTS="${NPM_REGISTRY_VERIFY_ATTEMPTS:-20}"
SLEEP_SECONDS="${NPM_REGISTRY_VERIFY_SLEEP_SECONDS:-3}"
MAX_SLEEP="${NPM_REGISTRY_VERIFY_MAX_SLEEP:-30}"
SLEEP_BIN="${NPM_REGISTRY_VERIFY_SLEEP_BIN:-sleep}"

delay="$SLEEP_SECONDS"
for ((attempt = 1; attempt <= MAX_ATTEMPTS; attempt++)); do
  published="$(npm view "${PKG}@${VERSION}" version 2>/dev/null || true)"
  if [[ "$published" == "$VERSION" ]]; then
    echo "  verified ${PKG}@${VERSION} on registry (attempt ${attempt}/${MAX_ATTEMPTS})"
    exit 0
  fi
  if [[ "$attempt" -lt "$MAX_ATTEMPTS" ]]; then
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
