#!/usr/bin/env bash
# Guards write-release-tag-gate.sh: the stamp names the SHA the gate tested, never a re-read HEAD.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WRITER_SRC="${WRITER_SRC:-$ROOT/scripts/ci/write-release-tag-gate.sh}"
GATE="$ROOT/scripts/ci/run-release-tag-local-ci.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

git init -q --bare "$TMP/origin.git"
git init -q "$TMP/work"
cd "$TMP/work"
git symbolic-ref HEAD refs/heads/main
git config user.name test
git config user.email test@example.com
git remote add origin "$TMP/origin.git"
mkdir -p scripts/ci
cp "$WRITER_SRC" scripts/ci/write-release-tag-gate.sh
git add scripts
git commit -q -m init
git push -q origin main
git fetch -q origin

STAMPS=".test-logs/release-tag-gate"
W="scripts/ci/write-release-tag-gate.sh"

# a. HEAD unchanged
T="$(git rev-parse HEAD)"
bash "$W" "$T" 0 0 0 >/dev/null 2>"$TMP/err" || fail "a: expected exit 0: $(cat "$TMP/err")"
[[ -f "$STAMPS/$T.json" ]] || fail "a: stamp missing"
grep -q "\"sha\": \"$T\"" "$STAMPS/$T.json" || fail "a: sha field mismatch"

# b. HEAD moved (incident shape): tested T, then main advanced and origin/main follows
rm -rf "$STAMPS"
echo change >> change.txt
git add change.txt
git commit -q -m change
git push -q origin main
git fetch -q origin
N="$(git rev-parse HEAD)"
rc=0
bash "$W" "$T" 0 0 0 >/dev/null 2>"$TMP/err" || rc=$?
[[ "$rc" == "1" ]] || fail "b: expected exit 1, got $rc"
grep -q "HEAD moved during the gate run (tested $T, now $N)" "$TMP/err" || fail "b: message missing: $(cat "$TMP/err")"
[[ ! -e "$STAMPS/$T.json" && ! -e "$STAMPS/$N.json" ]] || fail "b: stamp written"

# c. old 3-arg form and bad SHAs
rc=0
bash "$W" 0 0 0 >/dev/null 2>&1 || rc=$?
[[ "$rc" == "2" ]] || fail "c: 3-arg form expected exit 2, got $rc"
rc=0
bash "$W" abc123 0 0 0 >/dev/null 2>&1 || rc=$?
[[ "$rc" == "2" ]] || fail "c: short sha expected exit 2, got $rc"
rc=0
bash "$W" "$(printf 'z%.0s' $(seq 1 40))" 0 0 0 >/dev/null 2>&1 || rc=$?
[[ "$rc" == "2" ]] || fail "c: non-hex sha expected exit 2, got $rc"
[[ ! -e "$STAMPS/$N.json" ]] || fail "c: stamp written"

# d. existing checks still bite
rc=0
bash "$W" "$N" 1 0 0 >/dev/null 2>&1 || rc=$?
[[ "$rc" == "1" ]] || fail "d: non-zero quality expected exit 1, got $rc"
[[ ! -e "$STAMPS/$N.json" ]] || fail "d: stamp written on failed quality"
echo more >> change.txt
git commit -q -am unpushed
U="$(git rev-parse HEAD)"
rc=0
bash "$W" "$U" 0 0 0 >/dev/null 2>&1 || rc=$?
[[ "$rc" == "1" ]] || fail "d: unpushed HEAD expected exit 1, got $rc"
[[ ! -e "$STAMPS/$U.json" ]] || fail "d: stamp written for unpushed HEAD"

# e. the wrapper pins HEAD_SHA
grep -q 'write-release-tag-gate.sh "\$HEAD_SHA" 0 0 0' "$GATE" || fail "e: wrapper does not pass \"\$HEAD_SHA\""

echo "OK: write-release-tag-gate.test.sh"
