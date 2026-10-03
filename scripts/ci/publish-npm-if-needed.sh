#!/usr/bin/env bash
# Publish a package directory unless pkg@version is already on the npm registry.
#
# Usage:
#   bash scripts/ci/publish-npm-if-needed.sh <dir> <pkg> <version> [npm publish extras...]
#
# Deferred verify (platform binding packages only):
#   NPM_PUBLISH_DEFER_VERIFY=1 NPM_PUBLISH_PENDING_FILE=<file>
# After a successful `npm publish`, do ONE visibility check. If the package is not visible yet,
# record "<pkg> <version>" in the pending file and return 0 so the caller can publish the next
# platform package. The caller MUST then run wait-for-pending-npm-packages.sh before publishing
# anything that needs those packages resolvable at install time:
#   platform bindings -> (wait pending) -> bindings -> signaling -> sdk -> helpers
# Dependents (bindings, sdk, ...) are never published with deferral; they use the strict wait.
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
if [[ "${NPM_PUBLISH_DEFER_VERIFY:-}" == "1" ]]; then
  if bash "$ROOT/scripts/ci/npm-registry-visible.sh" "$PKG" "$VERSION"; then
    echo "  verified ${PKG}@${VERSION} on registry"
  else
    : "${NPM_PUBLISH_PENDING_FILE:?NPM_PUBLISH_PENDING_FILE required with NPM_PUBLISH_DEFER_VERIFY=1}"
    echo "${PKG} ${VERSION}" >>"$NPM_PUBLISH_PENDING_FILE"
    echo "  deferred verify of ${PKG}@${VERSION} (not visible yet; will wait before dependents)"
  fi
  exit 0
fi
bash "$ROOT/scripts/ci/wait-for-npm-package.sh" "$PKG" "$VERSION"
