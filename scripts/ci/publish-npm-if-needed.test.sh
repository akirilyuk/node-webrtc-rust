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

echo "publish-npm-if-needed.test.sh: all checks passed"
