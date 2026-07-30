#!/usr/bin/env bash
# True when build-main.yml completed successfully for the given commit on main.
# Uses GitHub REST via curl (no gh CLI).
set -euo pipefail

SHA="${1:-${GITHUB_SHA:-}}"
REPO="${GITHUB_REPOSITORY:-}"
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

if [[ -z "$SHA" || -z "$REPO" ]]; then
  echo "Usage: GITHUB_REPOSITORY=owner/repo check-main-ci-success.sh [sha]" >&2
  exit 1
fi

out="${GITHUB_OUTPUT:-/dev/stdout}"

if [[ -z "$TOKEN" ]]; then
  echo "No GITHUB_TOKEN — cannot query main CI" >&2
  echo "main_validated=false" >> "$out"
  exit 0
fi

# List successful workflow runs for this commit on main.
enc_sha="$(printf '%s' "$SHA" | python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.stdin.read(), safe=""))')"
url="https://api.github.com/repos/${REPO}/actions/workflows/build-main.yml/runs?branch=main&status=success&head_sha=${enc_sha}&per_page=5"
body="$(
  curl -fsSL \
    -H "Authorization: Bearer ${TOKEN}" \
    -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "$url" 2>/dev/null || true
)"

if [[ -z "$body" ]]; then
  echo "main CI query failed" >&2
  echo "main_validated=false" >> "$out"
  exit 0
fi

run_id="$(
  python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    print("")
    raise SystemExit
runs = data.get("workflow_runs") or []
for run in runs:
    if run.get("conclusion") == "success" and run.get("status") == "completed":
        path = str(run.get("path") or "")
        if path.endswith("build-main.yml") or not path:
            print(run.get("id") or "")
            break
' <<<"$body"
)"

if [[ -n "$run_id" ]]; then
  echo "Main CI validated commit ${SHA} (run ${run_id})"
  echo "main_validated=true" >> "$out"
else
  echo "No successful build-main.yml run for commit ${SHA}"
  echo "main_validated=false" >> "$out"
fi
