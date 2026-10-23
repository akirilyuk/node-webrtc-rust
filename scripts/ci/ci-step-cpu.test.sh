#!/usr/bin/env bash
# Guard for the per-step CPU accounting in ci-step.sh: with a readable cgroup cpu.stat (env
# CI_STEP_CPU_STAT_FILE points the script at a fake one) a step prints one extra
# "[ci-step]       cpu: <s>s over <wall>s (avg <n> cores) <label>" line after OK; without the file
# nothing extra is printed, and the OK line keeps its format (other tooling reads it).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STEP="$ROOT/scripts/ci/ci-step.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# 1. cpu.stat readable: usage goes 1.0 s -> 3.5 s while the step sleeps 1 s.
FAKE="$TMP/cpu.stat"
printf 'usage_usec 1000000\nuser_usec 900000\nsystem_usec 100000\n' >"$FAKE"
CI_STEP_CPU_STAT_FILE="$FAKE" bash "$STEP" cpu-step -- \
  bash -c 'printf "usage_usec 3500000\nuser_usec 3000000\nsystem_usec 500000\n" >"$1"; sleep 1' _ "$FAKE" \
  >"$TMP/cpu.out" 2>&1 || fail "cpu step failed: $(cat "$TMP/cpu.out")"
grep -Eq '^\[ci-step\]       cpu: 2\.5s over [0-9]+s \(avg [0-9]+\.[0-9]{2} cores\) cpu-step$' "$TMP/cpu.out" \
  || fail "expected 'cpu: 2.5s ... (avg N.NN cores) cpu-step'; got: $(cat "$TMP/cpu.out")"
grep -q 'cpu: 2.5s' "$TMP/cpu.out" || fail "cpu: 2.5s missing"
grep -q 'cores' "$TMP/cpu.out" || fail "cores missing"
[[ "$(grep -c 'cpu:' "$TMP/cpu.out")" == "1" ]] || fail "expected exactly one cpu line"
echo "ok: cpu line printed with the cpu.stat delta"

# 2. The OK line keeps its shape, and the cpu line comes right after it.
grep -Eq '^\[ci-step\] OK    cpu-step \([0-9]+s\)$' "$TMP/cpu.out" \
  || fail "OK line changed shape; got: $(cat "$TMP/cpu.out")"
grep -Eq '^\[ci-step\] START cpu-step$' "$TMP/cpu.out" || fail "START line changed shape"
ok_line="$(grep -n '^\[ci-step\] OK ' "$TMP/cpu.out" | cut -d: -f1)"
cpu_line="$(grep -n 'cpu:' "$TMP/cpu.out" | cut -d: -f1)"
[[ "$cpu_line" == "$((ok_line + 1))" ]] || fail "cpu line must directly follow the OK line"
echo "ok: OK/START lines unchanged, cpu line after OK"

# 3. cpu.stat missing: no cpu line, same OK line.
CI_STEP_CPU_STAT_FILE="$TMP/does-not-exist" bash "$STEP" no-cpu-step -- true >"$TMP/none.out" 2>&1 \
  || fail "step without cpu.stat failed"
if grep -q 'cpu:' "$TMP/none.out"; then fail "cpu line printed without cpu.stat: $(cat "$TMP/none.out")"; fi
grep -Eq '^\[ci-step\] OK    no-cpu-step \([0-9]+s\)$' "$TMP/none.out" || fail "OK line missing without cpu.stat"
echo "ok: no cpu line without cpu.stat"

# 4. Failing step: FAIL line unchanged, exit code propagated, cpu line on stderr after it.
printf 'usage_usec 0\n' >"$FAKE"
set +e
CI_STEP_CPU_STAT_FILE="$FAKE" bash "$STEP" bad-cpu-step -- bash -c 'exit 7' >"$TMP/bad.out" 2>&1
rc=$?
set -e
[[ "$rc" == "7" ]] || fail "failing step must exit 7, got $rc"
grep -Eq '^\[ci-step\] FAIL  bad-cpu-step \([0-9]+s, exit 7\)$' "$TMP/bad.out" || fail "FAIL line changed shape"
grep -Eq '^\[ci-step\]       cpu: 0\.0s over [0-9]+s \(avg (0\.00 cores|n/a)\) bad-cpu-step$' "$TMP/bad.out" \
  || fail "cpu line missing after FAIL: $(cat "$TMP/bad.out")"
echo "ok: failing step keeps its FAIL line and exit code, prints a cpu line"

echo "ci-step-cpu.test.sh: OK"
