#!/usr/bin/env bash
# Guard for the CI TURN server (ci.unpinned-image, test.ci-cluster-optional).
# coturn runs in-process in the integration container (docker/ci/Dockerfile.test installs the
# Ubuntu package; scripts/ci/turnserver-ci.sh starts it). This checks that:
# - any coturn/coturn image reference left in CI files is digest-pinned (none is fine);
# - the test image installs coturn;
# - turnserver-ci.sh passes the options Ubuntu's coturn 4.6 needs (--no-cli is required with
#   --allow-loopback-peers) and the credentials packages/sdk/tests/turn.test.ts uses;
# - the start waits a bounded time.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

while IFS= read -r line; do
  [[ -z "$line" ]] && continue
  case "$line" in
    *'coturn/coturn:'*'@sha256:'*) ;;
    *) fail "unpinned coturn/coturn image reference: $line" ;;
  esac
done < <(grep -rnE 'coturn/coturn[:@ ]' .github scripts/ci docker \
  --include='*.yml' --include='*.yaml' --include='*.sh' --include='Dockerfile*' 2>/dev/null \
  | grep -v 'turnserver-ci.test.sh' | grep -vE ':[0-9]+:\s*#' || true)

grep -qE 'apt-get install[^&]*\bcoturn\b' docker/ci/Dockerfile.test \
  || fail "docker/ci/Dockerfile.test must install the coturn package"

script=scripts/ci/turnserver-ci.sh
for opt in --no-cli --allow-loopback-peers --listening-port=3478 --lt-cred-mech \
  --user=testuser:testpass --no-tls --no-dtls; do
  grep -qF -- "$opt" "$script" || fail "$script does not pass $opt"
done
grep -qF 'within 10 s' "$script" || fail "$script must bound the start wait (10 s)"

turn_test=packages/sdk/tests/turn.test.ts
grep -qF "'testuser'" "$turn_test" && grep -qF "'testpass'" "$turn_test" \
  || fail "$turn_test credentials no longer match turnserver-ci.sh (testuser/testpass)"

echo "turnserver-ci.test.sh: OK"
