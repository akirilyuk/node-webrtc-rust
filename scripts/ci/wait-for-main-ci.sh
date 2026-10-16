#!/usr/bin/env bash
# Wait (bounded) for build-main.yml runs on a commit to finish, so release.yml `plan` can reuse
# the main run's native bundle and skip the integration suite. Waiting is an optimisation only:
# this script ALWAYS exits 0 and the existing checks (resolve-native-main-bundle.sh,
# check-main-ci-success.sh) decide what to reuse.
#
# Usage: wait-for-main-ci.sh [sha]            (default $GITHUB_SHA)
# Env:   GITHUB_REPOSITORY, GITHUB_TOKEN|GH_TOKEN,
#        MAIN_CI_WAIT_TIMEOUT_SEC (default 2700), MAIN_CI_POLL_SEC (default 30)
set -uo pipefail

SHA="${1:-${GITHUB_SHA:-}}"
REPO="${GITHUB_REPOSITORY:-}"
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
TIMEOUT="${MAIN_CI_WAIT_TIMEOUT_SEC:-2700}"
POLL="${MAIN_CI_POLL_SEC:-30}"

if [[ -z "$SHA" || -z "$REPO" ]]; then
  echo "wait-for-main-ci: no sha or GITHUB_REPOSITORY; not waiting"
  exit 0
fi
if [[ -z "$TOKEN" ]]; then
  echo "wait-for-main-ci: no GITHUB_TOKEN; not waiting"
  exit 0
fi

SHORT="${SHA:0:12}"
enc_sha="$(printf '%s' "$SHA" | python3 -I -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.stdin.read(), safe=""))')"
url="https://api.github.com/repos/${REPO}/actions/workflows/build-main.yml/runs?head_sha=${enc_sha}&per_page=10"

# Prints "<total> <in_progress>" or returns non-zero when the request or parse fails.
query() {
  local body
  body="$(
    curl -fsS --max-time 20 \
      -H "Authorization: Bearer ${TOKEN}" \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      "$url" 2>/dev/null
  )" || return 1
  python3 -I -c '
import json, sys
data = json.load(sys.stdin)
runs = data.get("workflow_runs") or []
print(len(runs), sum(1 for r in runs if r.get("status") != "completed"))
' <<<"$body" 2>/dev/null
}

while true; do
  if ! counts="$(query)"; then
    echo "wait-for-main-ci: build-main query failed; not waiting"
    exit 0
  fi
  total="${counts%% *}"
  pending="${counts##* }"
  if [[ "$total" == "0" ]]; then
    echo "wait-for-main-ci: no build-main run for ${SHORT}; not waiting"
    exit 0
  fi
  if [[ "$pending" == "0" ]]; then
    echo "build-main on ${SHORT} finished"
    exit 0
  fi
  if (( SECONDS >= TIMEOUT )); then
    echo "::warning::build-main on ${SHORT} still running after ${TIMEOUT}s; release continues without reusing it"
    exit 0
  fi
  echo "Waiting for build-main run(s) on ${SHORT}: ${pending} in progress (${SECONDS}s/${TIMEOUT}s)"
  sleep "$POLL"
done
