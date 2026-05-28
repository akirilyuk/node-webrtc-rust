#!/usr/bin/env bash
# Poll npm registry until pkg@version is visible after publish (eventual consistency).
set -euo pipefail

PKG="${1:?package name required}"
VERSION="${2:?version required}"
MAX_ATTEMPTS="${NPM_REGISTRY_VERIFY_ATTEMPTS:-12}"
SLEEP_SECONDS="${NPM_REGISTRY_VERIFY_SLEEP_SECONDS:-3}"

for (( attempt = 1; attempt <= MAX_ATTEMPTS; attempt++ )); do
  published="$(npm view "${PKG}@${VERSION}" version 2>/dev/null || true)"
  if [[ "$published" == "$VERSION" ]]; then
    echo "  verified ${PKG}@${VERSION} on registry (attempt ${attempt}/${MAX_ATTEMPTS})"
    exit 0
  fi
  if [[ "$attempt" -lt "$MAX_ATTEMPTS" ]]; then
    echo "  waiting for ${PKG}@${VERSION} on registry (attempt ${attempt}/${MAX_ATTEMPTS})..."
    sleep "$SLEEP_SECONDS"
  fi
done

echo "Not on npm registry after publish: ${PKG}@${VERSION} (after ${MAX_ATTEMPTS} attempts)" >&2
exit 1
