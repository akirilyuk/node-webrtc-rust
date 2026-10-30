#!/usr/bin/env bash
# Guards the Sherpa roundtrip groups in run-sherpa-example-ci.sh and its job pool:
#   1. every start:roundtrip* / start:record script of the example is in exactly one of
#      SHERPA_ROUNDTRIP_E2E_SERIAL / SHERPA_ROUNDTRIP_E2E_PARALLEL, except the reviewed not-in-CI
#      list (the load benchmarks, which were never part of the CI list); no duplicates;
#   2. the timing-sensitive scripts (barge-in, utterance timing, ...) are in the serial group;
#   3. the pool runs at most SHERPA_E2E_PARALLEL scripts at a time, reports a failing name after
#      draining, leaves no background process, keeps list order at width 1 and runs the serial
#      group after the parallel group. The pool is driven without real roundtrips through the hidden
#      hooks SHERPA_E2E_DRY_COMMAND (replaces the roundtrip; $1 = npm script) and SHERPA_E2E_ONLY;
#   4. run-sherpa-roundtrip-e2e.sh runs ensure_speaker_fixtures before start:roundtrip-noisy-bystander
#      (every path that runs it, including the stress workflow) and not before other scripts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
CI_SCRIPT="scripts/ci/run-sherpa-example-ci.sh"
PKG="examples/voice-agent-local-sherpa/package.json"

fail() { echo "sherpa-e2e-groups.test.sh: $*" >&2; exit 1; }

# Scripts that exist in package.json but were never in the CI list (load benchmarks; the CI variant
# is start:roundtrip-load-ci). Adding a name here needs a reason.
NOT_IN_CI=(start:roundtrip-load start:roundtrip-load-cached)

groups="$(bash "$CI_SCRIPT" groups)"
serial=() parallel=()
while read -r kind name; do
  case "$kind" in
    serial) serial+=("$name") ;;
    parallel) parallel+=("$name") ;;
    *) fail "unexpected groups line: $kind $name" ;;
  esac
done <<<"$groups"
[[ ${#serial[@]} -gt 0 && ${#parallel[@]} -gt 0 ]] || fail "both groups must be non-empty"

# 1. Membership.
dups="$(printf '%s\n' "${serial[@]}" "${parallel[@]}" | sort | uniq -d)"
[[ -z "$dups" ]] || fail "in more than one place: $dups"

pkg_scripts="$(grep -oE '^[[:space:]]*"start:(roundtrip[A-Za-z0-9-]*|record)"' "$PKG" | tr -d ' "' | sort)"
[[ -n "$pkg_scripts" ]] || fail "found no start:roundtrip*/start:record scripts in $PKG"
expected="$pkg_scripts"
for name in "${NOT_IN_CI[@]}"; do
  grep -qx "$name" <<<"$pkg_scripts" || fail "not-in-CI entry $name is not a script in $PKG (stale)"
  expected="$(grep -vx "$name" <<<"$expected")"
done
actual="$(printf '%s\n' "${serial[@]}" "${parallel[@]}" | sort)"
if [[ "$actual" != "$expected" ]]; then
  echo "expected (package.json minus not-in-CI):" >&2; echo "$expected" >&2
  echo "actual (serial + parallel):" >&2; echo "$actual" >&2
  fail "the two groups must cover exactly the roundtrip scripts that run in CI"
fi
# The CI list had 19 entries when the list was split; fewer means an entry was dropped.
[[ $(( ${#serial[@]} + ${#parallel[@]} )) -ge 19 ]] || fail "fewer than the 19 original roundtrip entries"

# step-timeouts (read by roundtrip-ci-step-budget.test.ts) covers both groups.
caps="$(bash "$CI_SCRIPT" step-timeouts | cut -d' ' -f1 | sort)"
[[ "$caps" == "$actual" ]] || fail "step-timeouts must list exactly the entries of both groups"

# 2. Timing-sensitive and CPU-heavy (10-session lid-multi) scripts stay serial.
for name in start:roundtrip-utterance-timing start:roundtrip-c1-timeouts start:roundtrip-slow-final \
  start:roundtrip-barge-in start:roundtrip-barge-in-buffered start:roundtrip-noisy-bystander start:roundtrip-counting-barge-recovery \
  start:roundtrip-concurrent-multi-client start:roundtrip-load-ci start:roundtrip-barge-replay \
  start:roundtrip-counting-echo-lid-multi; do
  printf '%s\n' "${serial[@]}" | grep -qx "$name" || fail "$name must be in the serial group"
done
for name in "${serial[@]}"; do
  case "$name" in
    start:roundtrip-utterance-timing | start:roundtrip-c1-timeouts | start:roundtrip-slow-final | \
      start:roundtrip-barge-in | start:roundtrip-barge-in-buffered | start:roundtrip-noisy-bystander | start:roundtrip-counting-barge-recovery | \
      start:roundtrip-concurrent-multi-client | start:roundtrip-load-ci | start:roundtrip-barge-replay | start:roundtrip-counting-echo-lid-multi) ;;
    *) fail "$name is in the serial group but not in the reviewed timing-sensitive list" ;;
  esac
done

# Width: SHERPA_E2E_PARALLEL, else 3 on CI and 1 elsewhere.
# 2, not 3: with ~1 core per roundtrip the voice pipeline starves and STT mishears (reproduced 2026-10-23).
[[ "$(env -u SHERPA_E2E_PARALLEL CI=true bash "$CI_SCRIPT" parallel-width)" == 2 ]] || fail "default width on CI must be 2 (CPU headroom per roundtrip)"
[[ "$(env -u SHERPA_E2E_PARALLEL -u CI bash "$CI_SCRIPT" parallel-width)" == 1 ]] || fail "default width off CI must be 1"
[[ "$(SHERPA_E2E_PARALLEL=5 CI=true bash "$CI_SCRIPT" parallel-width)" == 5 ]] || fail "SHERPA_E2E_PARALLEL must override"
if SHERPA_E2E_PARALLEL=0 bash "$CI_SCRIPT" parallel-width >/dev/null 2>&1; then fail "width 0 must be rejected"; fi

# 3. Pool behaviour.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
only_parallel="$(IFS=,; echo "${parallel[*]}")"
# Run the roundtrip phase with the dry hook. Usage: pool <width> <only> <dry command> <out file>; echoes the exit code.
pool() {
  local width="$1" only="$2" dry="$3" out="$4" rc=0
  SHERPA_E2E_PARALLEL="$width" SHERPA_E2E_ONLY="$only" SHERPA_E2E_DRY_COMMAND="$dry" \
    SHERPA_E2E_LOG_DIR="$tmp/logs" CI_STEP_TIMEOUT_DISABLE=1 \
    bash "$CI_SCRIPT" roundtrips >"$out" 2>&1 || rc=$?
  echo "$rc"
}

# Width 1 behaves like the serial runner: list order, plain step output, no pool header.
rm -rf "$tmp/logs"
rc="$(pool 1 "" 'echo "ran $1"' "$tmp/w1.out")"
[[ "$rc" == 0 ]] || { cat "$tmp/w1.out" >&2; fail "width 1 run failed (exit $rc)"; }
# start:roundtrip-barge-in runs a second time right after itself with SHERPA_VAD_PROVIDER=adaptive.
expected_order="$(printf 'ran %s\n' "${parallel[@]}" "${serial[@]}" | awk '{print} $2 == "start:roundtrip-barge-in" {print}')"
[[ "$(grep -E '^ran ' "$tmp/w1.out")" == "$expected_order" ]] || fail "width 1 must run the parallel group then the serial group in list order"
if grep -q '\[sherpa e2e parallel\]' "$tmp/w1.out"; then fail "width 1 must not use the pool"; fi
[[ ! -d "$tmp/logs" ]] || fail "width 1 must not write pool logs"

# 4. Every path that runs start:roundtrip-noisy-bystander gets its generated scenes: the stress
#    workflow calls run-sherpa-roundtrip-e2e.sh directly, so that script must run the fixture step
#    before npm (and only for that script). Driven with a copy of the script, a stub helper and a stub npm.
FIXTURE_HELPER="scripts/ci/ensure-speaker-fixtures.sh"
[[ -f "$FIXTURE_HELPER" ]] || fail "missing $FIXTURE_HELPER"
bash -c "ROOT='$ROOT'; source '$FIXTURE_HELPER'; declare -F ensure_speaker_fixtures >/dev/null" \
  || fail "$FIXTURE_HELPER must define ensure_speaker_fixtures"
for flag in '--main-levels -26' '--min-turn-gap-ms 2500' '--bystander-db -15' '--babble-clear-ms 1000' '--babble-clear-pull' '--only s3_test_m15'; do
  grep -qF -- "$flag" "$FIXTURE_HELPER" || fail "$FIXTURE_HELPER lost the scene arg: $flag"
done
# Re-roll mode: the stress workflow input reaches the helper, which then ignores the stamp and
# generates from an empty cache (fresh Piper roll per iteration, as on fresh CI containers).
grep -q 'SPEAKER_FIXTURES_REROLL' "$FIXTURE_HELPER" \
  || fail "$FIXTURE_HELPER must honour SPEAKER_FIXTURES_REROLL"
grep -q 'args+=(--cache "$cache_dir")' "$FIXTURE_HELPER" \
  || fail "$FIXTURE_HELPER re-roll must pass a fresh --cache dir"
grep -qE "SPEAKER_FIXTURES_REROLL: \\$\\{\\{ inputs.reroll_fixtures" .github/workflows/sherpa-e2e-stress.yml \
  || fail "sherpa-e2e-stress.yml must map input reroll_fixtures to SPEAKER_FIXTURES_REROLL"
grep -qE '^source "\$ROOT/scripts/ci/ensure-speaker-fixtures.sh"' "$CI_SCRIPT" \
  || fail "$CI_SCRIPT calls ensure_speaker_fixtures but does not source $FIXTURE_HELPER"
fr="$tmp/fixture-root"
mkdir -p "$fr/scripts/ci" "$fr/bin"
cp scripts/ci/run-sherpa-roundtrip-e2e.sh "$fr/scripts/ci/"
: >"$fr/scripts/export-sherpa-local-models.sh"
cat >"$fr/scripts/ci/ensure-speaker-fixtures.sh" <<'STUB'
ensure_speaker_fixtures() { echo fixtures >>"$FIXTURE_CALLS"; }
STUB
cat >"$fr/bin/npm" <<'STUB'
#!/usr/bin/env bash
echo "npm $2" >>"$FIXTURE_CALLS"
STUB
chmod +x "$fr/bin/npm"
for s in start:roundtrip-noisy-bystander start:roundtrip-counting; do
  : >"$tmp/fixture-calls"
  FIXTURE_CALLS="$tmp/fixture-calls" PATH="$fr/bin:$PATH" SHERPA_E2E_AUDIO_TAP=0 \
    bash "$fr/scripts/ci/run-sherpa-roundtrip-e2e.sh" "$s" >/dev/null 2>&1 \
    || fail "run-sherpa-roundtrip-e2e.sh $s failed with stubs"
  if [[ "$s" == start:roundtrip-noisy-bystander ]]; then
    expected="$(printf 'fixtures\nnpm %s' "$s")"
  else
    expected="npm $s"
  fi
  [[ "$(cat "$tmp/fixture-calls")" == "$expected" ]] \
    || fail "run-sherpa-roundtrip-e2e.sh $s: expected calls [$expected], got [$(tr '\n' ' ' <"$tmp/fixture-calls")] (noisy-bystander needs ensure_speaker_fixtures before npm)"
done

if ! bash -c '(( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3) ))'; then
  echo "sherpa-e2e-groups.test.sh: bash < 4.3 (no wait -n); skipping the width > 1 cases"
  echo "sherpa-e2e-groups.test.sh: OK (width 1 only)"
  exit 0
fi

# Width 3: ten one-second entries take about four seconds (not ten, not one), every entry has a log.
rm -rf "$tmp/logs"
start=$SECONDS
rc="$(pool 3 "$only_parallel" 'echo "ran $1"; sleep 1' "$tmp/w3.out")"
elapsed=$((SECONDS - start))
[[ "$rc" == 0 ]] || { cat "$tmp/w3.out" >&2; fail "width 3 run failed (exit $rc)"; }
[[ "$elapsed" -le 8 ]] || fail "width 3 took ${elapsed}s for ${#parallel[@]} one-second entries (expected about 4s)"
[[ "$elapsed" -ge 3 ]] || fail "width 3 took only ${elapsed}s: the pool ran more than 3 at a time"
for name in "${parallel[@]}"; do
  [[ -s "$tmp/logs/$name.log" ]] || fail "no log for $name"
  grep -Fq "==> [sherpa e2e parallel] $name (exit 0)" "$tmp/w3.out" || fail "no header for $name"
  grep -Fq "ran $name" "$tmp/logs/$name.log" || fail "log of $name lacks its own output"
done

# Width 3 on the full list: the whole parallel group is reported before the first serial script starts.
rm -rf "$tmp/logs"
rc="$(pool 3 "" 'echo "ran $1"' "$tmp/full.out")"
[[ "$rc" == 0 ]] || { cat "$tmp/full.out" >&2; fail "full width 3 run failed (exit $rc)"; }
last_header="$(grep -n '^==> \[sherpa e2e parallel\]' "$tmp/full.out" | tail -1 | cut -d: -f1)"
first_serial="$(grep -n "^ran ${serial[0]}\$" "$tmp/full.out" | head -1 | cut -d: -f1)"
[[ -n "$last_header" && -n "$first_serial" && "$last_header" -lt "$first_serial" ]] \
  || fail "the serial group must start after the parallel group has finished"
serial_expected="$(printf 'ran %s\n' "${serial[@]}" | awk '{print} $2 == "start:roundtrip-barge-in" {print}')"
[[ "$(grep -E '^ran ' "$tmp/full.out" | tail -n "$(printf '%s\n' "$serial_expected" | wc -l | tr -d ' ')")" == "$serial_expected" ]] \
  || fail "the serial group must run in list order"

# One failing entry: non-zero exit, the name is reported, the others still ran, nothing is left behind.
marker="sherpa-e2e-groups-marker-$$"
rm -rf "$tmp/logs"
rc="$(pool 3 "$only_parallel" "# $marker
if [[ \"\$1\" == start:record ]]; then echo failing-on-purpose; exit 3; fi
sleep 1" "$tmp/fail.out")"
[[ "$rc" != 0 ]] || fail "a failing entry must make the run fail"
grep -Fq "==> [sherpa e2e parallel] start:record (exit 3)" "$tmp/fail.out" || fail "failing entry header missing"
grep -Fq "FAILED (1/${#parallel[@]}): start:record (exit 3)" "$tmp/fail.out" || fail "failing name not reported"
grep -Fq "failing-on-purpose" "$tmp/fail.out" || fail "the failing entry's log must be printed"
for name in "${parallel[@]}"; do
  grep -Fq "==> [sherpa e2e parallel] $name (exit" "$tmp/fail.out" || fail "$name did not finish after a sibling failed"
done
if pgrep -f "$marker" >/dev/null 2>&1; then fail "background processes left after a failed run"; fi

# A signal mid-run kills the whole pool (needs pgrep to find the children).
if command -v pgrep >/dev/null 2>&1; then
  rm -rf "$tmp/logs"
  marker="sherpa-e2e-groups-kill-$$"
  SHERPA_E2E_PARALLEL=3 SHERPA_E2E_ONLY="$only_parallel" SHERPA_E2E_DRY_COMMAND="# $marker
sleep 30" SHERPA_E2E_LOG_DIR="$tmp/logs" CI_STEP_TIMEOUT_DISABLE=1 \
    bash "$CI_SCRIPT" roundtrips >"$tmp/kill.out" 2>&1 &
  runner=$!
  for _ in $(seq 1 50); do
    [[ "$(pgrep -f "$marker" | wc -l)" -ge 3 ]] && break
    sleep 0.2
  done
  pgrep -f "$marker" >/dev/null || fail "pool never started its members"
  kill -TERM "$runner" 2>/dev/null || true
  wait "$runner" 2>/dev/null || true
  sleep 1
  if pgrep -f "$marker" >/dev/null 2>&1; then
    pkill -f "$marker" 2>/dev/null || true
    fail "SIGTERM left pool members running"
  fi
fi

echo "sherpa-e2e-groups.test.sh: OK"
