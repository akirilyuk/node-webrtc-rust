#!/usr/bin/env bash
# Print the CHANGELOG section for a release version (for GitHub Release body).
# Usage: bash scripts/changelog-release-body.sh 0.3.0
set -euo pipefail

VERSION="${1:?usage: changelog-release-body.sh <version>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHANGELOG="${ROOT}/CHANGELOG.md"

if [[ ! -f "$CHANGELOG" ]]; then
  echo "Missing ${CHANGELOG}" >&2
  exit 1
fi

# Print from "## [VERSION]" until the next "## [" heading (exclusive).
awk -v ver="$VERSION" '
  /^## \[/ {
    if (found) exit
    if ($0 ~ "^## \\[" ver "\\]") found = 1
  }
  found { print }
' "$CHANGELOG"
