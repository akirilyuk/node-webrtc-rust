#!/usr/bin/env bash
# Guards that every `needs.<job>.outputs.<name>` used in a workflow is declared under that job's
# `outputs:` map (an undeclared output is silently "" and made the PR quality job skip on every PR).
# Usage: workflow-job-outputs.test.sh [workflow.yml ...]   (default: .github/workflows/*.yml)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

check_file() {
  local f="$1"
  awk '
    # Top-level job key: exactly two spaces of indent under "jobs:"
    /^jobs:/ { injobs=1; next }
    /^[^ #]/ { injobs=0 }
    injobs && /^  [A-Za-z0-9_-]+:[ ]*$/ { job=$1; sub(/:$/, "", job); inout=0; next }
    injobs && /^    outputs:[ ]*$/ { inout=1; next }
    injobs && inout && /^      [A-Za-z0-9_-]+:/ { k=$1; sub(/:$/, "", k); declared[job SUBSEP k]=1; next }
    injobs && inout && !/^      / && !/^[ ]*$/ { inout=0 }
    {
      line=$0
      while (match(line, /needs\.[A-Za-z0-9_-]+\.outputs\.[A-Za-z0-9_]+/)) {
        ref=substr(line, RSTART, RLENGTH)
        used[ref]=1
        line=substr(line, RSTART+RLENGTH)
      }
    }
    END {
      bad=0
      for (r in used) {
        split(r, p, ".")
        if (!((p[2] SUBSEP p[4]) in declared)) {
          printf "%s: %s is used but not declared in job %s outputs\n", FILENAME, r, p[2] > "/dev/stderr"
          bad=1
        }
      }
      exit bad
    }
  ' "$f"
}

files=("$@")
if [[ ${#files[@]} -eq 0 ]]; then
  files=(.github/workflows/*.yml)
fi

rc=0
for f in "${files[@]}"; do
  check_file "$f" || rc=1
done

if [[ "$rc" -ne 0 ]]; then
  exit 1
fi

# Self-probe: dropping run_quality from `changes.outputs` must be detected.
if [[ $# -eq 0 ]]; then
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"' EXIT
  grep -v '^      run_quality:' .github/workflows/build.yml >"$tmp"
  if check_file "$tmp" 2>/dev/null; then
    echo "probe failed: removing run_quality from changes.outputs was not detected" >&2
    exit 1
  fi
  echo "ok: probe (run_quality removed) is detected"
fi

echo "workflow-job-outputs.test.sh: all checks passed"
