#!/usr/bin/env bash
# Align main with a version just published to npm and refresh package-lock.json.
#
# Used by the Release workflow after the publish job (platform bindings exist on npm).
# Run on a checkout of main; does not commit — the workflow opens a PR via create-pull-request.
#
# Usage: bash scripts/ci/post-release-sync-main-package-lock.sh <version>
#
# Docs: scripts/RELEASE.md#package-lockjson-after-release
# CI: .github/workflows/release.yml job sync-main-package-lock
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="${1:-}"

if [[ -z "$VERSION" ]]; then
  echo "Usage: $0 <version>" >&2
  exit 1
fi

cd "$ROOT"

if [[ "${GITHUB_REF_TYPE:-}" == "tag" && -n "${GITHUB_REF_NAME:-}" ]]; then
  TAG_VERSION="${GITHUB_REF_NAME#release/}"
  if [[ "$TAG_VERSION" != "$VERSION" ]]; then
    echo "WARN: tag version ($TAG_VERSION) != argument ($VERSION)" >&2
  fi
fi

echo "==> Post-release sync on main @ $VERSION (packages must be on npm)"
bash "$ROOT/scripts/ci/bump-workspace-versions.sh" "$VERSION"

if git diff --quiet package-lock.json package.json packages/ examples/ 2>/dev/null; then
  echo "==> No workspace or lockfile changes — PR step will be skipped"
else
  echo "==> Changes to commit:"
  git diff --stat package-lock.json package.json packages/ examples/ || true
fi
