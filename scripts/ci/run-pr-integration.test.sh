#!/usr/bin/env bash
# Guard (test.time-budget-creep): run-pr-integration.sh compiles every cargo test binary in its own
# "cargo test build" step BEFORE the per-crate cargo steps, so their wall-clock caps measure test time
# and not a cache-dependent compile (release-prep PR #475, 2026-10-19: `cargo speech` hit its 180 s
# cap while compiling on a cold cache).
#
# Checks, by parsing the script:
#   1. a "cargo test build" ci-step exists and comes before every later `cargo test` / `cargo check` step;
#   2. every package (-p) and manifest (--manifest-path) a later step uses appears in the build function;
#   3. the build line uses the same --features and covers every --lib / --test target of the later steps
#      (so the later step finds identical artifacts and does not compile again).
# RUN_PR_INTEGRATION_SCRIPT overrides the script under test (used by the negative cases below).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${RUN_PR_INTEGRATION_SCRIPT:-$ROOT/scripts/ci/run-pr-integration.sh}"

check() {
  python3 -I - "$1" <<'PY'
import re, sys

path = sys.argv[1]
lines = open(path).read().splitlines()

def fail(msg):
    print(f"FAIL: {msg}", file=sys.stderr)
    sys.exit(1)

step_idx = next((i for i, l in enumerate(lines) if '"cargo test build"' in l and 'CI_STEP' in l), None)
if step_idx is None:
    fail('no `"$CI_STEP" ... "cargo test build"` step in ' + path)

start = next((i for i, l in enumerate(lines) if re.match(r'\s*cargo_test_build\(\)\s*\{', l)), None)
if start is None:
    fail("no cargo_test_build() function in " + path)
end = next((i for i in range(start + 1, len(lines)) if re.match(r'\s*\}\s*$', lines[i])), None)
if end is None:
    fail("cargo_test_build() is not closed")
if not start < end < step_idx:
    fail("cargo_test_build() must be defined before the `cargo test build` step that runs it")

def parse(line):
    cmd = re.split(r'\s--\s', line.strip(), maxsplit=1)[0]  # drop libtest args after `--`
    t = cmd.split()
    kind = t[1]  # test | check
    t = [x for x in t[2:] if x != '--no-run']
    pkgs, manifest, feats, tests, lib, other = [], None, [], [], False, []
    i = 0
    while i < len(t):
        a = t[i]
        if a == '-p': pkgs.append(t[i + 1]); i += 2
        elif a == '--manifest-path': manifest = t[i + 1]; i += 2
        elif a == '--features': feats.append(t[i + 1]); i += 2
        elif a == '--test': tests.append(t[i + 1]); i += 2
        elif a == '--lib': lib = True; i += 1
        else: other.append(a); i += 1
    key = (kind, tuple(sorted(pkgs)), manifest)
    return key, set(feats), set(tests), lib, bool(tests) or lib

cargo_re = re.compile(r'^\s+cargo (test|check)\s')
build = {}
for l in lines[start + 1:end]:
    if cargo_re.match(l):
        key, feats, tests, lib, targeted = parse(l)
        build[key] = (feats, tests, lib, targeted)

later = {}
for i, l in enumerate(lines):
    if start <= i <= end or not cargo_re.match(l):
        continue
    if i < step_idx:
        fail(f"line {i + 1}: `{l.strip()}` runs before the `cargo test build` step")
    key, feats, tests, lib, targeted = parse(l)
    cur = later.setdefault(key, [set(), set(), False, False])
    cur[0] |= feats; cur[1] |= tests; cur[2] = cur[2] or lib; cur[3] = cur[3] or not targeted

if not later:
    fail("no cargo test/check steps found after the build step (parser out of date?)")

for key, (feats, tests, lib, bare) in later.items():
    kind, pkgs, manifest = key
    name = ' '.join(pkgs) if pkgs else manifest
    if key not in build:
        fail(f"package/manifest `{name}` is used by a later cargo {kind} step but not built in cargo_test_build()")
    bfeats, btests, blib, btargeted = build[key]
    if bfeats != feats:
        fail(f"`{name}`: build features {sorted(bfeats)} != later step features {sorted(feats)}")
    if bare and btargeted:
        fail(f"`{name}`: later step has no explicit targets but the build line limits targets")
    if not bare:
        missing = sorted(tests - btests)
        if missing:
            fail(f"`{name}`: test target(s) {missing} used later are not built in cargo_test_build()")
        if lib and not blib:
            fail(f"`{name}`: --lib used later is not built in cargo_test_build()")
print(f"ok: {len(later)} cargo step group(s) are built before they run")
PY
}

expect_fail() {
  local label="$1" needle="$2" file="$3" out
  if out="$(check "$file" 2>&1)"; then
    echo "FAIL: guard did not reject: $label" >&2
    exit 1
  fi
  grep -q -- "$needle" <<<"$out" || { echo "FAIL: $label: expected '$needle', got: $out" >&2; exit 1; }
  echo "ok: rejects $label"
}

check "$SCRIPT"

# Negative cases on temp copies.
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

grep -v 'cargo test --no-run -p node-webrtc-rust-speech$' "$SCRIPT" >"$TMP/no-speech.sh"
expect_fail "a package missing from the build step" "node-webrtc-rust-speech" "$TMP/no-speech.sh"

sed 's/--no-run -p node-webrtc-rust-vendor-groq --features live/--no-run -p node-webrtc-rust-vendor-groq/' "$SCRIPT" >"$TMP/no-feature.sh"
expect_fail "a build line without the later step's --features" "node-webrtc-rust-vendor-groq" "$TMP/no-feature.sh"

sed 's/ --test vendor_test$//' "$SCRIPT" >"$TMP/no-target.sh"
if cmp -s "$SCRIPT" "$TMP/no-target.sh"; then echo "FAIL: no-target mutation did not apply" >&2; exit 1; fi
expect_fail "a test target missing from the build step" "not built in cargo_test_build" "$TMP/no-target.sh"

sed 's/"cargo test build"/"cargo compile"/' "$SCRIPT" >"$TMP/no-step.sh"
expect_fail "a script without the build step" "cargo test build" "$TMP/no-step.sh"

echo "run-pr-integration.test.sh: OK"
