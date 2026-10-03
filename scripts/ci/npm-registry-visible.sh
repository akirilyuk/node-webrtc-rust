#!/usr/bin/env bash
# Exit 0 if pkg@version is visible on the npm registry, 1 otherwise (single check, no waiting).
#
# Two documented lookups, either one is enough:
#   1. `npm view <pkg>@<version> version`
#   2. GET https://registry.npmjs.org/<pkg>/<version> -> HTTP 200
#      (npm registry "get package version" endpoint; scoped names use %2F for the slash)
#
# Env: NPM_REGISTRY_VERIFY_CURL_BIN (default curl; tests set it to `false` to stay offline).
set -uo pipefail

PKG="${1:?package name required}"
VERSION="${2:?version required}"
CURL_BIN="${NPM_REGISTRY_VERIFY_CURL_BIN:-curl}"

published="$(npm view "${PKG}@${VERSION}" version 2>/dev/null || true)"
if [[ "$published" == "$VERSION" ]]; then
  exit 0
fi

if command -v "$CURL_BIN" >/dev/null 2>&1; then
  encoded="${PKG//\//%2F}"
  code="$("$CURL_BIN" -s -o /dev/null -w '%{http_code}' --max-time 20 \
    "https://registry.npmjs.org/${encoded}/${VERSION}" 2>/dev/null || true)"
  if [[ "$code" == "200" ]]; then
    exit 0
  fi
fi
exit 1
