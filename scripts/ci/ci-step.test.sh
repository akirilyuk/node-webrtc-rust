#!/usr/bin/env bash
# Guard for the ci-step.sh time-budget early warning (test.time-budget-creep): a passing step that used
# at least 75 % of its --timeout prints WARN (+ a GitHub annotation and step summary line); one under
# 75 %, one without a cap, and the FAIL path behave as before.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STEP="$ROOT/scripts/ci/ci-step.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# Whole-second elapsed (floor): cap 4 s, sleep 3.2 s is always >= 3 s = 75 %.
# Two runs in parallel (with and without GitHub Actions env) keep the test at ~3.2 s.
(
  GITHUB_ACTIONS=1 GITHUB_STEP_SUMMARY="$TMP/summary.md" \
    bash "$STEP" --timeout 4 slow-step -- sleep 3.2 >"$TMP/slow-gh.out" 2>&1
  echo $? >"$TMP/slow-gh.rc"
) &
pid_gh=$!
(
  env -u GITHUB_ACTIONS -u GITHUB_STEP_SUMMARY \
    bash "$STEP" --timeout 4 slow-step -- sleep 3.2 >"$TMP/slow-plain.out" 2>&1
  echo $? >"$TMP/slow-plain.rc"
) &
pid_plain=$!
wait "$pid_gh" "$pid_plain"

[[ "$(cat "$TMP/slow-gh.rc")" == "0" ]] || fail "slow step must still pass (rc $(cat "$TMP/slow-gh.rc"))"
grep -Eq '^\[ci-step\] WARN slow-step used [0-9]+s of 4s \([0-9]+%\)$' "$TMP/slow-gh.out" \
  || fail "expected WARN line for a step at >=75% of its cap; got: $(cat "$TMP/slow-gh.out")"
grep -q '^::warning title=CI step near its time budget::slow-step [0-9]*s/4s$' "$TMP/slow-gh.out" \
  || fail "expected ::warning annotation under GITHUB_ACTIONS"
grep -q 'slow-step' "$TMP/summary.md" || fail "expected a GITHUB_STEP_SUMMARY line"
echo "ok: step at >=75% of its cap warns (stdout, annotation, summary)"

[[ "$(cat "$TMP/slow-plain.rc")" == "0" ]] || fail "slow step (plain env) must still pass"
grep -q '^\[ci-step\] WARN slow-step' "$TMP/slow-plain.out" || fail "WARN missing outside GitHub Actions"
if grep -q '^::warning' "$TMP/slow-plain.out"; then fail "::warning printed without GITHUB_ACTIONS"; fi
echo "ok: no annotation outside GitHub Actions"

# Under 75 %: cap 4 s, instant command.
GITHUB_ACTIONS=1 bash "$STEP" --timeout 4 quick-step -- true >"$TMP/quick.out" 2>&1 || fail "quick step failed"
grep -q 'OK    quick-step' "$TMP/quick.out" || fail "quick step missing OK line"
if grep -q 'WARN\|::warning' "$TMP/quick.out"; then fail "quick step must not warn: $(cat "$TMP/quick.out")"; fi
echo "ok: step under 75% does not warn"

# No cap: nothing to compare against.
bash "$STEP" nocap-step -- true >"$TMP/nocap.out" 2>&1 || fail "uncapped step failed"
if grep -q 'WARN' "$TMP/nocap.out"; then fail "uncapped step must not warn"; fi
echo "ok: uncapped step does not warn"

# FAIL path unchanged: non-zero exit is propagated, FAIL printed, no WARN.
set +e
bash "$STEP" --timeout 4 bad-step -- bash -c 'exit 7' >"$TMP/bad.out" 2>&1
rc=$?
set -e
[[ "$rc" == "7" ]] || fail "failing step must exit 7, got $rc"
grep -q 'FAIL  bad-step (.*exit 7)' "$TMP/bad.out" || fail "FAIL line missing"
if grep -q 'WARN' "$TMP/bad.out"; then fail "failing step must not print WARN"; fi
echo "ok: failing step still fails without WARN"

# Timeout path unchanged: exit 124 with the cap hint, no WARN.
if command -v timeout >/dev/null 2>&1 || command -v gtimeout >/dev/null 2>&1; then
  set +e
  bash "$STEP" --timeout 1 hung-step -- sleep 5 >"$TMP/hung.out" 2>&1
  rc=$?
  set -e
  [[ "$rc" == "124" ]] || fail "capped step must exit 124, got $rc"
  grep -q 'FAIL  hung-step (.*exit 124)' "$TMP/hung.out" || fail "FAIL line missing for timeout"
  if grep -q 'WARN' "$TMP/hung.out"; then fail "timed-out step must not print WARN"; fi
  echo "ok: timed-out step still exits 124 without WARN"
fi

echo "ci-step.test.sh: OK"
