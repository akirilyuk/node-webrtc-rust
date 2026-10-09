#!/usr/bin/env bash
# Unit tests for wait-for-npm-package.sh (no network).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# Fake npm: first N views miss, then hit VERSION.
cat >"$TMP/npm" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# args: view pkg@version version
state_file="${FAKE_NPM_STATE:?}"
want="${FAKE_NPM_VERSION:?}"
misses="${FAKE_NPM_MISSES:?}"
count=0
if [[ -f "$state_file" ]]; then
  count="$(cat "$state_file")"
fi
count=$((count + 1))
echo "$count" >"$state_file"
if [[ "$count" -le "$misses" ]]; then
  exit 1
fi
echo "$want"
EOF
chmod +x "$TMP/npm"

# Record sleep delays instead of waiting.
cat >"$TMP/sleep" <<'EOF'
#!/usr/bin/env bash
echo "$1" >>"${FAKE_SLEEP_LOG:?}"
EOF
chmod +x "$TMP/sleep"

export PATH="$TMP:$PATH"
export FAKE_NPM_VERSION=0.8.1
export NPM_REGISTRY_VERIFY_CURL_BIN=false # offline: skip registry GET
export FAKE_SLEEP_LOG="$TMP/sleeps"
export NPM_REGISTRY_VERIFY_SLEEP_BIN="$TMP/sleep"
export NPM_REGISTRY_VERIFY_ATTEMPTS=8
export NPM_REGISTRY_VERIFY_SLEEP_SECONDS=3
export NPM_REGISTRY_VERIFY_MAX_SLEEP=30

# Immediate hit — no sleeps.
export FAKE_NPM_STATE="$TMP/count-hit"
export FAKE_NPM_MISSES=0
: >"$FAKE_SLEEP_LOG"
bash scripts/ci/wait-for-npm-package.sh @node-webrtc-rust/sdk 0.8.1 >/dev/null
[[ ! -s "$FAKE_SLEEP_LOG" ]] || fail "expected no sleep on first-try hit"

# 3 misses then hit — delays 3, 6, 12
export FAKE_NPM_STATE="$TMP/count-backoff"
export FAKE_NPM_MISSES=3
: >"$FAKE_SLEEP_LOG"
bash scripts/ci/wait-for-npm-package.sh @node-webrtc-rust/sdk 0.8.1 >/dev/null
got="$(tr '\n' ' ' <"$FAKE_SLEEP_LOG" | sed 's/[[:space:]]*$//')"
[[ "$got" == "3 6 12" ]] || fail "expected exponential 3 6 12, got: $got"

# Cap at MAX_SLEEP (5 misses → sleeps 3 6 12 24 30)
export FAKE_NPM_STATE="$TMP/count-cap"
export FAKE_NPM_MISSES=5
: >"$FAKE_SLEEP_LOG"
bash scripts/ci/wait-for-npm-package.sh @node-webrtc-rust/sdk 0.8.1 >/dev/null
got="$(tr '\n' ' ' <"$FAKE_SLEEP_LOG" | sed 's/[[:space:]]*$//')"
[[ "$got" == "3 6 12 24 30" ]] || fail "expected cap 30 after 24, got: $got"

# Exhaust attempts
export FAKE_NPM_STATE="$TMP/count-fail"
export FAKE_NPM_MISSES=99
export NPM_REGISTRY_VERIFY_ATTEMPTS=3
: >"$FAKE_SLEEP_LOG"
if bash scripts/ci/wait-for-npm-package.sh @node-webrtc-rust/sdk 0.8.1 >/dev/null 2>"$TMP/err"; then
  fail "expected failure when never visible"
fi
grep -q "Not on npm registry after publish" "$TMP/err" || fail "missing failure message"
got="$(tr '\n' ' ' <"$FAKE_SLEEP_LOG" | sed 's/[[:space:]]*$//')"
[[ "$got" == "3 6" ]] || fail "fail path should sleep attempts-1 times (3 6), got: $got"

# Defaults: slow-but-eventual visibility inside the ~30 min window (0.9.18: visible after ~17 min).
unset NPM_REGISTRY_VERIFY_ATTEMPTS NPM_REGISTRY_VERIFY_SLEEP_SECONDS NPM_REGISTRY_VERIFY_MAX_SLEEP
export FAKE_NPM_STATE="$TMP/count-slow"
export FAKE_NPM_MISSES=25
: >"$FAKE_SLEEP_LOG"
bash scripts/ci/wait-for-npm-package.sh @node-webrtc-rust/bindings-linux-arm64-gnu 0.8.1 >/dev/null \
  || fail "default window must tolerate 25 misses"
total="$(awk '{s+=$1} END{print s+0}' "$FAKE_SLEEP_LOG")"
[[ "$total" -ge 900 && "$total" -le 1800 ]] || fail "expected 15-30 min of sleeps before visible, got ${total}s"

# Defaults: never visible -> fails after a ~30 min budget (sum of sleeps).
export FAKE_NPM_STATE="$TMP/count-never"
export FAKE_NPM_MISSES=9999
: >"$FAKE_SLEEP_LOG"
if bash scripts/ci/wait-for-npm-package.sh @node-webrtc-rust/sdk 0.8.1 >/dev/null 2>"$TMP/err"; then
  fail "expected failure when never visible (defaults)"
fi
total="$(awk '{s+=$1} END{print s+0}' "$FAKE_SLEEP_LOG")"
[[ "$total" -ge 1700 && "$total" -le 1900 ]] || fail "default budget should be ~30 min, got ${total}s"

# Registry GET fallback: npm view misses but GET /{pkg}/{version} returns 200 (scope slash encoded).
cat >"$TMP/curl" <<'EOF2'
#!/usr/bin/env bash
echo "$*" >>"${FAKE_CURL_LOG:?}"
printf '200'
EOF2
chmod +x "$TMP/curl"
export FAKE_CURL_LOG="$TMP/curl.log"
export NPM_REGISTRY_VERIFY_CURL_BIN=curl
export FAKE_NPM_STATE="$TMP/count-curl"
export FAKE_NPM_MISSES=9999
: >"$FAKE_SLEEP_LOG"
bash scripts/ci/wait-for-npm-package.sh @node-webrtc-rust/sdk 0.8.1 >/dev/null || fail "registry GET 200 should count as visible"
grep -q 'https://registry.npmjs.org/@node-webrtc-rust%2Fsdk/0.8.1' "$FAKE_CURL_LOG" || fail "unexpected registry URL: $(cat "$FAKE_CURL_LOG")"

# Deadline: MAX_ATTEMPTS=1000 must not extend past NPM_PUBLISH_DEADLINE_EPOCH (fake clock, probe +600 s).
mkdir -p "$TMP/ci"
cp scripts/ci/wait-for-npm-package.sh "$TMP/ci/"
cat >"$TMP/ci/npm-registry-visible.sh" <<'EOS'
#!/usr/bin/env bash
echo $(( $(cat "$FAKE_CLOCK") + 600 )) >"$FAKE_CLOCK"
exit 1
EOS
cat >"$TMP/now" <<'EOS'
#!/usr/bin/env bash
cat "${FAKE_CLOCK:?}"
EOS
cat >"$TMP/sleep-clock" <<'EOS'
#!/usr/bin/env bash
echo "$1" >>"${FAKE_SLEEP_LOG:?}"
echo $(( $(cat "$FAKE_CLOCK") + $1 )) >"$FAKE_CLOCK"
EOS
chmod +x "$TMP/now" "$TMP/sleep-clock"
export FAKE_CLOCK="$TMP/clock"
echo 1000 >"$FAKE_CLOCK"; : >"$FAKE_SLEEP_LOG"
if NPM_PUBLISH_DEADLINE_EPOCH=4000 NPM_REGISTRY_VERIFY_ATTEMPTS=1000 \
  NPM_REGISTRY_VERIFY_NOW_BIN="$TMP/now" NPM_REGISTRY_VERIFY_SLEEP_BIN="$TMP/sleep-clock" \
  bash "$TMP/ci/wait-for-npm-package.sh" @node-webrtc-rust/sdk 0.8.1 >/dev/null 2>"$TMP/err"; then
  fail "expected failure at deadline"
fi
grep -q "deadline reached" "$TMP/err" || fail "missing 'deadline reached' message"
end="$(cat "$FAKE_CLOCK")"
[[ "$end" -le $((4000 + 600)) ]] || fail "clock ${end} ran past deadline+600"
awk '{s+=$1} END {exit !(s <= 3000)}' "$FAKE_SLEEP_LOG" || fail "slept more than the time to deadline"

echo "wait-for-npm-package.test.sh: all checks passed"
