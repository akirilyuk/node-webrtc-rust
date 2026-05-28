#!/usr/bin/env bash
# True when build-main.yml completed successfully for the given commit on main.
set -euo pipefail

SHA="${1:-${GITHUB_SHA:-}}"
REPO="${GITHUB_REPOSITORY:-}"

if [[ -z "$SHA" || -z "$REPO" ]]; then
  echo "Usage: GITHUB_REPOSITORY=owner/repo check-main-ci-success.sh [sha]" >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "main_validated=false" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
fi

RUN_ID="$(
  gh run list \
    --repo "$REPO" \
    --workflow=build-main.yml \
    --commit="$SHA" \
    --status=success \
    --branch=main \
    --limit=1 \
    --json databaseId \
    --jq '.[0].databaseId // empty' 2>/dev/null || true
)"

if [[ -n "$RUN_ID" ]]; then
  echo "Main CI validated commit ${SHA} (run ${RUN_ID})"
  echo "main_validated=true" >> "${GITHUB_OUTPUT:-/dev/stdout}"
else
  echo "No successful build-main.yml run for commit ${SHA}"
  echo "main_validated=false" >> "${GITHUB_OUTPUT:-/dev/stdout}"
fi
