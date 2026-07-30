#!/usr/bin/env bash
# Static checks for Alpine GNU tar cache plumbing.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

SCRIPT=scripts/ci/ensure-gnu-tar-alpine.sh
INSTALL=scripts/ci/install-alpine-native-toolchain.sh
LINUX=.github/actions/ci-build-native-linux/action.yml
CACHE=.github/actions/native-binding-cache/action.yml
HOST=.github/actions/ci-build-native-host/action.yml

chmod +x "$SCRIPT"

grep -q 'apk add --no-cache' "$INSTALL"
grep -qE '(^|[[:space:]])tar([[:space:]]|$)' "$INSTALL" || {
  echo "FAIL: install-alpine-native-toolchain must install GNU tar package" >&2
  exit 1
}
grep -q 'gnu tar' "$INSTALL" || {
  echo "FAIL: install script must verify /bin/tar is GNU tar" >&2
  exit 1
}

grep -q 'ensure-gnu-tar-alpine.sh' "$LINUX" || {
  echo "FAIL: linux native build must ensure GNU tar before cache on musl" >&2
  exit 1
}
grep -q 'ensure-gnu-tar-alpine.sh' "$CACHE" || {
  echo "FAIL: native-binding-cache must ensure GNU tar before restore on musl" >&2
  exit 1
}

if grep -q 'napi-zig\|napi-rs-nodejs' "$HOST" "$LINUX"; then
  echo "FAIL: zig toolchain cache must not remain after compile recipe dropped --zig" >&2
  exit 1
fi

# Non-Alpine hosts should no-op.
if ! grep -qi alpine /etc/os-release 2>/dev/null; then
  bash "$SCRIPT"
  echo "ok: ensure-gnu-tar-alpine no-ops off Alpine"
fi

echo "ensure-gnu-tar-alpine.test.sh: all checks passed"
