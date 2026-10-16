#!/usr/bin/env bash
# Offline test for wait-for-main-ci.sh: a fake curl returns fixture bodies from a counter file.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/ci/wait-for-main-ci.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin"

# Fake curl: FAKE_CURL_BODIES is a ':'-separated list of fixture names under $TMP; call N returns
# the Nth (last one repeats). FAKE_CURL_FAIL=1 exits 22. Every call increments $TMP/count.
cat >"$TMP/bin/curl" <<'FAKE'
#!/usr/bin/env bash
n=$(( $(cat "$FAKE_DIR/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$FAKE_DIR/count"
if [[ "${FAKE_CURL_FAIL:-}" == "1" ]]; then exit 22; fi
IFS=':' read -r -a bodies <<<"$FAKE_CURL_BODIES"
idx=$(( n - 1 ))
(( idx >= ${#bodies[@]} )) && idx=$(( ${#bodies[@]} - 1 ))
cat "$FAKE_DIR/${bodies[$idx]}.json"
FAKE
chmod +x "$TMP/bin/curl"

echo '{"workflow_runs":[{"status":"in_progress"}]}' >"$TMP/running.json"
echo '{"workflow_runs":[{"status":"completed","conclusion":"success"}]}' >"$TMP/done.json"
echo '{"workflow_runs":[]}' >"$TMP/none.json"

calls() { cat "$TMP/count" 2>/dev/null || echo 0; }

# run_case <bodies> [extra env...] -> sets OUT, RC, ELAPSED
run_case() {
  local bodies="$1"
  shift
  rm -f "$TMP/count"
  local start=$SECONDS
  set +e
  # Unset any ambient token first (BSD env needs -u before NAME=value); NO_TOKEN=1 leaves it unset.
  local tok=(GITHUB_TOKEN=x)
  [[ "${NO_TOKEN:-}" == "1" ]] && tok=()
  OUT="$(env -u GITHUB_TOKEN -u GH_TOKEN PATH="$TMP/bin:$PATH" FAKE_DIR="$TMP" FAKE_CURL_BODIES="$bodies" \
    MAIN_CI_POLL_SEC=0.1 GITHUB_REPOSITORY=o/r ${tok[@]+"${tok[@]}"} "$@" \
    bash "$SCRIPT" 0123456789abcdef0123 2>&1)"
  RC=$?
  set -e
  ELAPSED=$((SECONDS - start))
}

fail() {
  echo "FAIL: $1" >&2
  echo "$OUT" >&2
  exit 1
}

# in progress twice then completed
run_case "running:running:done"
[[ $RC -eq 0 ]] || fail "progress-then-done: exit $RC"
grep -q 'finished' <<<"$OUT" || fail "progress-then-done: no 'finished'"
[[ "$(calls)" == "3" ]] || fail "progress-then-done: curl calls $(calls), want 3"
echo "ok: waits while in progress, returns when completed"

# never completes
run_case "running" MAIN_CI_WAIT_TIMEOUT_SEC=1
[[ $RC -eq 0 ]] || fail "timeout: exit $RC"
(( ELAPSED <= 5 )) || fail "timeout: took ${ELAPSED}s"
grep -q 'still running after 1s' <<<"$OUT" || fail "timeout: no warning"
echo "ok: bounded by MAIN_CI_WAIT_TIMEOUT_SEC"

# no runs
run_case "none"
[[ $RC -eq 0 ]] || fail "no-runs: exit $RC"
[[ "$(calls)" == "1" ]] || fail "no-runs: curl calls $(calls), want 1"
echo "ok: no runs for the sha returns immediately"

# curl fails
run_case "running" FAKE_CURL_FAIL=1
[[ $RC -eq 0 ]] || fail "curl-fail: exit $RC"
[[ "$(calls)" == "1" ]] || fail "curl-fail: curl calls $(calls), want 1"
echo "ok: request failure returns immediately"

# no token
NO_TOKEN=1 run_case "running"
[[ $RC -eq 0 ]] || fail "no-token: exit $RC"
[[ "$(calls)" == "0" ]] || fail "no-token: curl calls $(calls), want 0"
echo "ok: no token never calls the API"

echo "wait-for-main-ci.test.sh: OK"
