#!/usr/bin/env bash
# Publish a package directory unless pkg@version is already on the npm registry.
#
# Usage:
#   bash scripts/ci/publish-npm-if-needed.sh <dir> <pkg> <version> [npm publish extras...]
#
# Examples:
#   bash scripts/ci/publish-npm-if-needed.sh packages/helpers/ @node-webrtc-rust/helpers 0.8.1 --ignore-scripts
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DIR="${1:?package directory required}"
PKG="${2:?package name required}"
VERSION="${3:?version required}"
shift 3

if [[ -f "${DIR}/package.json" ]]; then
  pkg_dir="$DIR"
elif [[ -f "${DIR}package.json" ]]; then
  pkg_dir="$DIR"
else
  echo "publish-npm-if-needed: no package.json in ${DIR}" >&2
  exit 1
fi

published="$(npm view "${PKG}@${VERSION}" version 2>/dev/null || true)"
if [[ "$published" == "$VERSION" ]]; then
  echo "  skip ${PKG}@${VERSION} (already on registry)"
  exit 0
fi

echo "  publishing ${PKG}@${VERSION}"
(
  cd "$pkg_dir"
  npm publish --access public "$@"
)
bash "$ROOT/scripts/ci/wait-for-npm-package.sh" "$PKG" "$VERSION"
