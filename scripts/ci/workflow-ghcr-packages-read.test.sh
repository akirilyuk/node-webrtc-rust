#!/usr/bin/env bash
# Guard: a job that runs in a ghcr.io `container:` pulls its image with GITHUB_TOKEN. A job-level
# `permissions:` map replaces the workflow-level one, so it must itself contain `packages` (read),
# otherwise ghcr answers 403, the ARC `-workflow` pod stays Pending and the job fails with
# "pod failed to come online". Effective permissions = job map, else top-level map; absent,
# `read-all` and `write-all` are fine.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

# No YAML library on the quality host (it runs before npm ci), so this reads the fixed block layout
# the workflows use: top-level `permissions:`, `jobs:` at column 0, job names at 2 spaces, job keys
# at 4 spaces.
python3 -I - <<'PY'
import glob, re, sys

def val(rest):
    rest = rest.split(" #")[0].strip()
    return rest

bad = []
for f in sorted(glob.glob(".github/workflows/*.y*ml")):
    lines = open(f).read().split("\n")
    top = None  # None=absent, "str"=scalar, set=map keys
    in_jobs = False
    job = None
    jobs = {}
    i = 0
    n = len(lines)

    def block_keys(start, indent):
        keys, j = set(), start
        while j < n:
            ln = lines[j]
            if ln.strip() == "" or ln.lstrip().startswith("#"):
                j += 1
                continue
            ind = len(ln) - len(ln.lstrip())
            if ind <= indent:
                break
            if ind == indent + 2:
                m = re.match(r"\s*([A-Za-z0-9_-]+):", ln)
                if m:
                    keys.add(m.group(1))
            j += 1
        return keys

    while i < n:
        ln = lines[i]
        m0 = re.match(r"^permissions:\s*(.*)$", ln)
        if m0:
            r = val(m0.group(1))
            top = "str" if r else block_keys(i + 1, 0)
        elif re.match(r"^jobs:\s*$", ln):
            in_jobs = True
        elif in_jobs and re.match(r"^\S", ln) and not ln.startswith("#"):
            in_jobs = False
        elif in_jobs:
            mj = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", ln)
            if mj:
                job = mj.group(1)
                jobs[job] = {"image": None, "perms": None}
            elif job:
                mc = re.match(r"^    container:\s*(.*)$", ln)
                if mc:
                    r = val(mc.group(1))
                    if r:
                        jobs[job]["image"] = r
                    else:
                        j = i + 1
                        while j < n and (lines[j].startswith("      ") or lines[j].strip() == ""):
                            mi = re.match(r"^      image:\s*(.*)$", lines[j])
                            if mi:
                                jobs[job]["image"] = val(mi.group(1))
                            j += 1
                mp = re.match(r"^    permissions:\s*(.*)$", ln)
                if mp:
                    r = val(mp.group(1))
                    jobs[job]["perms"] = "str" if r else block_keys(i + 1, 4)
        i += 1
    for name, j in jobs.items():
        if not j["image"] or "ghcr.io" not in j["image"]:
            continue
        perms = j["perms"] if j["perms"] is not None else top
        if perms is None or perms == "str":
            continue
        if "packages" not in perms:
            bad.append(f"{f}: job '{name}' (container {j['image']})")

if bad:
    print("FAIL: ghcr.io container jobs without `packages` in effective permissions:", file=sys.stderr)
    for b in bad:
        print("  " + b, file=sys.stderr)
    sys.exit(1)
print("ok: every ghcr.io container job has packages permission")
PY
