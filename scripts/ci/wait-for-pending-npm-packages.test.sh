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

echo "wait-for-pending-npm-packages.test.sh: all checks passed"
