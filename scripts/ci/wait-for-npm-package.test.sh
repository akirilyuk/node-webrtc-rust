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

echo "wait-for-npm-package.test.sh: all checks passed"
