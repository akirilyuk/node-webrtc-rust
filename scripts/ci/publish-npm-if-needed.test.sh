#!/usr/bin/env bash
# Unit tests for publish-npm-if-needed.sh (no network / no real npm publish).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

mkdir -p "$TMP/pkg"
echo '{"name":"@node-webrtc-rust/helpers","version":"0.8.1"}' >"$TMP/pkg/package.json"

cat >"$TMP/npm" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log="${FAKE_NPM_LOG:?}"
printf '%s\n' "$*" >>"$log"
if [[ "${1:-}" == "view" ]]; then
  if [[ "${FAKE_NPM_VIEW:-}" == "hit" ]]; then
    echo "${FAKE_NPM_VERSION:-0.8.1}"
    exit 0
  fi
  exit 1
fi
if [[ "${1:-}" == "publish" ]]; then
  echo "FAKE_PUBLISH $*"
  exit 0
fi
exit 1
EOF
chmod +x "$TMP/npm"

# wait-for-npm-package after a real publish: make view succeed immediately.
export PATH="$TMP:$PATH"
export FAKE_NPM_LOG="$TMP/npm.log"
export FAKE_NPM_VERSION=0.8.1
export NPM_REGISTRY_VERIFY_ATTEMPTS=2
export NPM_REGISTRY_VERIFY_SLEEP_BIN=true
export NPM_REGISTRY_VERIFY_CURL_BIN=false # offline: skip registry GET

: >"$FAKE_NPM_LOG"
export FAKE_NPM_VIEW=hit
out="$(bash scripts/ci/publish-npm-if-needed.sh "$TMP/pkg" @node-webrtc-rust/helpers 0.8.1 --ignore-scripts)"
echo "$out" | grep -q "skip @node-webrtc-rust/helpers@0.8.1" || fail "expected skip, got: $out"
if grep -q '^publish ' "$FAKE_NPM_LOG"; then
  fail "npm publish must not run when already on registry"
fi

: >"$FAKE_NPM_LOG"
export FAKE_NPM_VIEW=miss
# After publish, wait script views again — flip to hit on 2nd view via counter.
cat >"$TMP/npm" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
log="${FAKE_NPM_LOG:?}"
printf '%s\n' "$*" >>"$log"
count=0
if [[ -f "${FAKE_NPM_COUNT:?}" ]]; then
  count="$(cat "$FAKE_NPM_COUNT")"
fi
count=$((count + 1))
echo "$count" >"$FAKE_NPM_COUNT"
if [[ "${1:-}" == "view" ]]; then
  # First view (pre-publish check) misses; later views (wait) hit.
  if [[ "$count" -eq 1 ]]; then
    exit 1
  fi
  echo "${FAKE_NPM_VERSION:-0.8.1}"
  exit 0
fi
if [[ "${1:-}" == "publish" ]]; then
  echo "FAKE_PUBLISH"
  exit 0
fi
exit 1
EOF
chmod +x "$TMP/npm"
export FAKE_NPM_COUNT="$TMP/count"
: >"$FAKE_NPM_COUNT"
out="$(bash scripts/ci/publish-npm-if-needed.sh "$TMP/pkg/" @node-webrtc-rust/helpers 0.8.1 --ignore-scripts)"
echo "$out" | grep -q "publishing @node-webrtc-rust/helpers@0.8.1" || fail "expected publish, got: $out"
grep -q '^publish --access public --ignore-scripts' "$FAKE_NPM_LOG" || fail "expected npm publish --access public --ignore-scripts"

# --- Deferred verify + ordering (fake registry with per-package visibility) ---------------
# Registry model: a package is "published" once `npm publish` ran in its dir; it becomes
# visible only after FAKE_LAG_<n> view calls following publish (n = sanitized name).
mkdir -p "$TMP/arm" "$TMP/x64" "$TMP/bindings"
echo '{"name":"@scope/p-arm","version":"1.0.0"}' >"$TMP/arm/package.json"
echo '{"name":"@scope/p-x64","version":"1.0.0"}' >"$TMP/x64/package.json"
echo '{"name":"@scope/bindings","version":"1.0.0"}' >"$TMP/bindings/package.json"
cat >"$TMP/npm" <<'EOF2'
#!/usr/bin/env bash
set -euo pipefail
dir="${FAKE_REG:?}"; mkdir -p "$dir"
if [[ "$1" == "publish" ]]; then
  name="$(sed -n 's/.*"name":"\([^"]*\)".*/\1/p' package.json | tr '/@-' '___')"
  # Dependents must not be published before their deps are visible.
  if [[ "$name" == "_scope_bindings" ]]; then
    for dep in _scope_p_arm _scope_p_x64; do
      [[ "$(cat "$dir/$dep.vis" 2>/dev/null || echo 0)" == "1" ]] || { echo "dep $dep not visible at publish of bindings" >&2; echo "ORDER-VIOLATION" >>"$dir/events"; }
    done
  fi
  echo "publish $name" >>"$dir/events"
  echo 0 >"$dir/$name.views"
  exit 0
fi
if [[ "$1" == "view" ]]; then
  spec="$2"; name="$(echo "${spec%@*}" | tr '/@-' '___')"
  [[ -f "$dir/$name.views" ]] || exit 1
  n=$(( $(cat "$dir/$name.views") + 1 )); echo "$n" >"$dir/$name.views"
  lag_var="FAKE_LAG_${name}"; lag="${!lag_var:-0}"
  if [[ "$n" -gt "$lag" ]]; then echo 1 >"$dir/$name.vis"; echo "${spec##*@}"; exit 0; fi
  exit 1
fi
exit 1
EOF2
chmod +x "$TMP/npm"
export FAKE_REG="$TMP/reg"; rm -rf "$FAKE_REG"
export NPM_PUBLISH_PENDING_FILE="$TMP/pending"; : >"$NPM_PUBLISH_PENDING_FILE"
export NPM_REGISTRY_VERIFY_ATTEMPTS=3
export FAKE_SLEEP_LOG="$TMP/sleeps2"; : >"$FAKE_SLEEP_LOG"
cat >"$TMP/sleep" <<'EOF2'
#!/usr/bin/env bash
echo "$1" >>"${FAKE_SLEEP_LOG:?}"
EOF2
chmod +x "$TMP/sleep"
export NPM_REGISTRY_VERIFY_SLEEP_BIN="$TMP/sleep"
export FAKE_LAG__scope_p_arm=6 # arm tarball is slow: not visible for the first 6 views

# arm is slow -> deferred, flow continues to x64 (not blocked, no sleeping).
NPM_PUBLISH_DEFER_VERIFY=1 bash scripts/ci/publish-npm-if-needed.sh "$TMP/arm" @scope/p-arm 1.0.0 >/dev/null
grep -q '^@scope/p-arm 1.0.0$' "$NPM_PUBLISH_PENDING_FILE" || fail "slow platform pkg should be recorded as pending"
[[ ! -s "$FAKE_SLEEP_LOG" ]] || fail "deferred verify must not sleep"
NPM_PUBLISH_DEFER_VERIFY=1 bash scripts/ci/publish-npm-if-needed.sh "$TMP/x64" @scope/p-x64 1.0.0 >/dev/null
grep -q 'publish _scope_p_x64' "$FAKE_REG/events" || fail "next platform package must be published after a deferred one"
grep -q 'x64' "$NPM_PUBLISH_PENDING_FILE" && fail "visible package must not be pending"

# Strict (no defer) publish of a dependent is not affected: still waits (and fails if never visible).
# Drain pending within budget: arm becomes visible after a few polls.
NPM_REGISTRY_VERIFY_TOTAL_SECONDS=600 bash scripts/ci/wait-for-pending-npm-packages.sh >/dev/null \
  || fail "pending drain should succeed once arm is visible"
[[ ! -s "$NPM_PUBLISH_PENDING_FILE" ]] || fail "pending file should be cleared after drain"
bash scripts/ci/publish-npm-if-needed.sh "$TMP/bindings" @scope/bindings 1.0.0 >/dev/null
grep -q ORDER-VIOLATION "$FAKE_REG/events" && fail "bindings published before platform deps were visible"

# A deferred package that never appears makes the drain fail (and names it).
rm -rf "$FAKE_REG"; : >"$NPM_PUBLISH_PENDING_FILE"; : >"$FAKE_SLEEP_LOG"
export FAKE_LAG__scope_p_arm=999999
NPM_PUBLISH_DEFER_VERIFY=1 bash scripts/ci/publish-npm-if-needed.sh "$TMP/arm" @scope/p-arm 1.0.0 >/dev/null
if NPM_REGISTRY_VERIFY_TOTAL_SECONDS=300 bash scripts/ci/wait-for-pending-npm-packages.sh >/dev/null 2>"$TMP/err2"; then
  fail "drain must fail when a deferred package never appears"
fi
grep -q 'Not on npm registry after publish: @scope/p-arm@1.0.0' "$TMP/err2" || fail "failure must name the missing package"
total="$(awk '{s+=$1} END{print s+0}' "$FAKE_SLEEP_LOG")"
[[ "$total" -ge 300 ]] || fail "drain should spend the full budget before failing, slept ${total}s"

echo "publish-npm-if-needed.test.sh: all checks passed"
