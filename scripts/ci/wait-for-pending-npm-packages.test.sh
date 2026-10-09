#!/usr/bin/env bash
# Guard tests for wait-for-pending-npm-packages.sh budget (no network).
# Class: ci.partial-publish-registry-lag (0.9.34: ~90 min lag outran a 30 min budget).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

SCRIPT=scripts/ci/wait-for-pending-npm-packages.sh

# (a) default budget parsed from the script is >= 2 h.
default="$(sed -n 's/^TOTAL="\${NPM_REGISTRY_VERIFY_TOTAL_SECONDS:-\([0-9]*\)}"$/\1/p' "$SCRIPT")"
[[ -n "$default" ]] || fail "could not parse default TOTAL from $SCRIPT"
[[ "$default" -ge 7200 ]] || fail "default budget ${default}s must be >= 7200"

# Fake checker: never visible. Fake sleep: record delays.
mkdir -p "$TMP/ci"
cp scripts/ci/wait-for-pending-npm-packages.sh "$TMP/ci/"
cat >"$TMP/ci/npm-registry-visible.sh" <<'EOS'
#!/usr/bin/env bash
exit 1
EOS
cat >"$TMP/sleep" <<'EOS'
#!/usr/bin/env bash
echo "$1" >>"${FAKE_SLEEP_LOG:?}"
EOS
chmod +x "$TMP/sleep"
export FAKE_SLEEP_LOG="$TMP/sleeps"
export NPM_REGISTRY_VERIFY_SLEEP_BIN="$TMP/sleep"
export NPM_PUBLISH_PENDING_FILE="$TMP/pending"

sum_sleeps() { awk '{s+=$1} END {print s+0}' "$FAKE_SLEEP_LOG"; }

# Unset env: sleeps add up to >= 7200 s and no sleep exceeds 60 s.
echo "@scope/p 1.0.0" >"$NPM_PUBLISH_PENDING_FILE"; : >"$FAKE_SLEEP_LOG"
env -u NPM_REGISTRY_VERIFY_TOTAL_SECONDS bash "$TMP/ci/wait-for-pending-npm-packages.sh" >/dev/null 2>&1 \
  && fail "never-visible package must fail"
total="$(sum_sleeps)"
[[ "$total" -ge 7200 ]] || fail "default run slept ${total}s, expected >= 7200"
max="$(sort -n "$FAKE_SLEEP_LOG" | tail -1)"
[[ "$max" -le 60 ]] || fail "poll interval ${max}s exceeds 60s cap"

# (b) env override still works.
echo "@scope/p 1.0.0" >"$NPM_PUBLISH_PENDING_FILE"; : >"$FAKE_SLEEP_LOG"
NPM_REGISTRY_VERIFY_TOTAL_SECONDS=300 bash "$TMP/ci/wait-for-pending-npm-packages.sh" >/dev/null 2>&1 \
  && fail "override run must fail"
total="$(sum_sleeps)"
[[ "$total" -ge 300 && "$total" -lt 400 ]] || fail "override 300s slept ${total}s"

# (c) Publish release job timeout > budget/60 + 30 minutes.
timeout="$(awk '
  /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { injob = 0 }
  /^    name: Publish release[[:space:]]*$/ { injob = 1 }
  injob && /^    timeout-minutes:/ { print $2; exit }
' .github/workflows/release.yml)"
[[ -n "$timeout" ]] || fail "Publish release job has no timeout-minutes"
need=$((default / 60 + 30))
[[ "$timeout" -gt "$need" ]] || fail "timeout-minutes ${timeout} must exceed budget/60+30 = ${need}"

# --- Wall-clock deadline (NPM_PUBLISH_DEADLINE_EPOCH) -----------------------------------------
# Fake clock in a file; fake sleep advances it; fake probe advances it by 600 s and counts calls.
cat >"$TMP/now" <<'EOS'
#!/usr/bin/env bash
cat "${FAKE_CLOCK:?}"
EOS
cat >"$TMP/sleep-clock" <<'EOS'
#!/usr/bin/env bash
echo "$1" >>"${FAKE_SLEEP_LOG:?}"
echo $(( $(cat "$FAKE_CLOCK") + $1 )) >"$FAKE_CLOCK"
EOS
cat >"$TMP/ci/npm-registry-visible.sh" <<'EOS'
#!/usr/bin/env bash
echo $(( $(cat "$FAKE_CLOCK") + 600 )) >"$FAKE_CLOCK"
echo probe >>"${FAKE_PROBE_LOG:?}"
exit 1
EOS
chmod +x "$TMP/now" "$TMP/sleep-clock"
export FAKE_CLOCK="$TMP/clock" FAKE_PROBE_LOG="$TMP/probes"
export NPM_REGISTRY_VERIFY_NOW_BIN="$TMP/now"
export NPM_REGISTRY_VERIFY_SLEEP_BIN="$TMP/sleep-clock"

# (deadline a) probes advance the clock by 600; polling stops at the deadline, never sleeps past it.
echo 1000 >"$FAKE_CLOCK"; : >"$FAKE_SLEEP_LOG"; : >"$FAKE_PROBE_LOG"
echo "@scope/p 1.0.0" >"$NPM_PUBLISH_PENDING_FILE"
NPM_PUBLISH_DEADLINE_EPOCH=4000 NPM_REGISTRY_VERIFY_TOTAL_SECONDS=999999 \
  bash "$TMP/ci/wait-for-pending-npm-packages.sh" >/dev/null 2>"$TMP/err" \
  && fail "deadline run must fail"
grep -q "deadline reached" "$TMP/err" || fail "missing 'deadline reached' message"
end="$(cat "$FAKE_CLOCK")"
[[ "$end" -le $((4000 + 600)) ]] || fail "clock ${end} ran past deadline+600"
awk '{s+=$1} END {exit !(s <= 3000)}' "$FAKE_SLEEP_LOG" || fail "slept more than the time to deadline"

# (deadline b) deadline already passed: exactly one probe round, exit 1.
echo 5000 >"$FAKE_CLOCK"; : >"$FAKE_SLEEP_LOG"; : >"$FAKE_PROBE_LOG"
echo "@scope/p 1.0.0" >"$NPM_PUBLISH_PENDING_FILE"
NPM_PUBLISH_DEADLINE_EPOCH=4000 \
  bash "$TMP/ci/wait-for-pending-npm-packages.sh" >/dev/null 2>"$TMP/err" \
  && fail "expired deadline must fail"
[[ "$(wc -l <"$FAKE_PROBE_LOG" | tr -d ' ')" == "1" ]] || fail "expected exactly one probe round"
[[ ! -s "$FAKE_SLEEP_LOG" ]] || fail "must not sleep after the deadline"
grep -q "deadline reached" "$TMP/err" || fail "missing 'deadline reached' message (expired)"

# (e) release.yml: the publish step shares one budget and defers every dependent's verify.
RY=.github/workflows/release.yml
budget="$(sed -n 's/.*NPM_PUBLISH_BUDGET_SECONDS="\${NPM_PUBLISH_BUDGET_SECONDS:-\([0-9]*\)}".*/\1/p' "$RY" | head -1)"
[[ -n "$budget" ]] || fail "could not parse NPM_PUBLISH_BUDGET_SECONDS default from $RY"
[[ $((timeout * 60)) -ge $((budget + 1200)) ]] || fail "timeout-minutes ${timeout} must cover budget ${budget}s + 1200s"
step="$(awk '/- name: Publish npm packages/ {f=1; next} f && /^      - name:/ {exit} f' "$RY")"
lineno() { printf '%s\n' "$step" | grep -n -- "$1" | head -1 | cut -d: -f1; }
b_line="$(lineno '"@node-webrtc-rust/bindings"')"
h_line="$(lineno '"@node-webrtc-rust/helpers"')"
[[ -n "$b_line" && -n "$h_line" ]] || fail "could not find bindings/helpers publish lines"
before="$(printf '%s\n' "$step" | head -n "$b_line" | grep -c 'wait-for-pending-npm-packages.sh')"
after="$(printf '%s\n' "$step" | tail -n +"$h_line" | grep -c 'wait-for-pending-npm-packages.sh')"
[[ "$before" == "1" ]] || fail "expected exactly one wait-for-pending call before bindings publish, got ${before}"
[[ "$after" == "1" ]] || fail "expected exactly one wait-for-pending call after helpers publish, got ${after}"
for pkg in bindings signaling sdk helpers; do
  printf '%s\n' "$step" | grep -- "\"@node-webrtc-rust/${pkg}\"" | grep -q 'NPM_PUBLISH_DEFER_VERIFY=1' \
    || fail "@node-webrtc-rust/${pkg} publish line must carry NPM_PUBLISH_DEFER_VERIFY=1"
done
printf '%s\n' "$step" | grep -q 'export NPM_PUBLISH_DEADLINE_EPOCH=' || fail "publish step must export NPM_PUBLISH_DEADLINE_EPOCH"

echo "wait-for-pending-npm-packages.test.sh: all checks passed"
