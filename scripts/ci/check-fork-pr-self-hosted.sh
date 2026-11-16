#!/usr/bin/env bash
# Guard (ci.fork-pr-on-self-hosted): a workflow that a pull request can trigger must not run a job on a
# self-hosted runner (ARC scale sets, eu-ci hosts) for a fork PR. A fork PR runs its own code on the
# runner once a maintainer approves it; on a persistent host that code can plant a backdoor that later
# steals the secrets of trusted jobs.
#
# Scope: every .github/workflows/*.yml triggered by `pull_request` / `pull_request_target`, plus every
# reusable workflow they call (`uses: ./.github/workflows/X.yml`, transitively). In those files, every
# job with a `runs-on` that is not a literal GitHub-hosted label (ubuntu-*, macos-*, windows-*) needs
# either an `if:` containing `head.repo.full_name == github.repository`, or a `runs-on` expression that
# sends fork PRs to a hosted label (`head.repo.full_name != github.repository` plus a hosted label), for
# a required check that must still report (a skipped required check counts as passing).
#
# Usage: check-fork-pr-self-hosted.sh [workflows-dir]   (default: .github/workflows of this repo)
# Exit 0 = ok; exit 1 = violations (file + job named); exit 2 = usage/parse problem.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DIR="${1:-$ROOT/.github/workflows}"
[[ -d "$DIR" ]] || { echo "check-fork-pr-self-hosted: no such directory: $DIR" >&2; exit 2; }

exec python3 - "$DIR" <<'PY'
import re
import sys
from pathlib import Path

wf_dir = Path(sys.argv[1])
HOSTED = re.compile(r"^(ubuntu|macos|windows)-[A-Za-z0-9._-]+$")
SAME_REPO = "head.repo.full_name == github.repository"
FORK_REPO = "head.repo.full_name != github.repository"


def strip(line):
    return line.split(" #", 1)[0].rstrip() if not line.lstrip().startswith("#") else ""


def blocks(lines, indent):
    """Map key -> (inline value, body lines) for keys at exactly `indent` spaces."""
    out, key, val, body = {}, None, "", []
    pat = re.compile(r"^" + " " * indent + r"([A-Za-z0-9_.\-\"']+):\s*(.*)$")
    for raw in lines:
        line = strip(raw)
        if not line.strip():
            continue
        lead = len(line) - len(line.lstrip(" "))
        m = pat.match(line) if lead == indent else None
        if m:
            if key is not None:
                out[key] = (val, body)
            key, val, body = m.group(1).strip("\"'"), m.group(2).strip(), []
        elif key is not None and lead > indent:
            body.append(line.strip())
    if key is not None:
        out[key] = (val, body)
    return out


def text(entry):
    val, body = entry
    return " ".join([val] + body).strip()


def parse(path):
    lines = path.read_text(encoding="utf-8").splitlines()
    top = blocks(lines, 0)
    on_text = text(top["on"]) if "on" in top else ""
    pr = bool(re.search(r"\bpull_request(_target)?\b", on_text))
    jobs = {}
    if "jobs" in top:
        job_lines = top["jobs"][1]
        # re-indent: body lines were stripped, so re-read the raw job section for exact indents
        start = next(i for i, l in enumerate(lines) if l.startswith("jobs:"))
        end = len(lines)
        for i in range(start + 1, len(lines)):
            if lines[i].strip() and not lines[i].startswith((" ", "#")):
                end = i
                break
        raw = lines[start + 1:end]
        for name, entry in blocks(raw, 2).items():
            jobs[name] = blocks(entry[1] and raw_job(raw, name) or [], 4)
    return pr, jobs


def raw_job(raw, name):
    out, inside = [], False
    for line in raw:
        if re.match(r"^  " + re.escape(name) + r":\s*$", line):
            inside = True
            continue
        if inside and re.match(r"^  [A-Za-z0-9_.\-]+:", line):
            break
        if inside:
            out.append(line)
    return out


files = {p.name: parse(p) for p in sorted(wf_dir.glob("*.yml"))}
checked = {n for n, (pr, _) in files.items() if pr}
changed = True
while changed:
    changed = False
    for n in list(checked):
        for job in files[n][1].values():
            m = re.search(r"\./\.github/workflows/([A-Za-z0-9_.\-]+\.yml)", text(job["uses"])) if "uses" in job else None
            if m and m.group(1) in files and m.group(1) not in checked:
                checked.add(m.group(1))
                changed = True

bad = []
for n in sorted(checked):
    for name, job in files[n][1].items():
        if "runs-on" not in job:
            continue
        ro = text(job["runs-on"])
        tokens = re.findall(r"['\"]([^'\"]+)['\"]|^([A-Za-z0-9._-]+)$", ro)
        labels = [a or b for a, b in tokens]
        if labels and all(HOSTED.match(l) for l in labels) and not re.search(r"\b(vars|matrix|inputs)\.", ro):
            continue  # only literal GitHub-hosted labels
        cond = text(job["if"]) if "if" in job else ""
        if SAME_REPO in cond:
            continue
        if FORK_REPO in ro and any(HOSTED.match(l) for l in labels):
            continue
        bad.append((n, name, ro))

if bad:
    for n, name, ro in bad:
        print(f"FAIL {n}: job '{name}' runs on a non-GitHub-hosted runner ({ro}) in a pull-request workflow "
              f"without `if: ... {SAME_REPO}` (fork PRs must not run on self-hosted runners)", file=sys.stderr)
    sys.exit(1)
print(f"ok: {len(checked)} pull-request workflow file(s) checked, no ungated self-hosted job")
PY
