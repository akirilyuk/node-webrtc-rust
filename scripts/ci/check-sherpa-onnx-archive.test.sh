#!/usr/bin/env bash
# Contract tests for check-sherpa-onnx-archive.sh (static version lockstep + runtime archive check).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/scripts/ci/check-sherpa-onnx-archive.sh"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/check-sherpa-archive-XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# Fake repo root with the three files the static check reads.
make_root() {
  local dir="$1" lock="$2" docker="$3" musl="$4"
  mkdir -p "$dir/docker/ci" "$dir/scripts/ci"
  printf '[[package]]\nname = "sherpa-onnx-sys"\nversion = "%s"\nsource = "registry"\n' "$lock" >"$dir/Cargo.lock"
  printf 'FROM ubuntu:24.04\nARG SHERPA_ONNX_VERSION=%s\n' "$docker" >"$dir/docker/ci/Dockerfile"
  printf '#!/usr/bin/env bash\nSHERPA_VERSION="${SHERPA_ONNX_VERSION:-%s}"\n' "$musl" >"$dir/scripts/ci/build-sherpa-onnx-musl-libs.sh"
}

# 1. Real repo: Dockerfile and musl default equal Cargo.lock.
bash "$SCRIPT" --static >/dev/null || fail "static check fails on the real repo"

# 2. Dockerfile behind Cargo.lock: named mismatch error.
make_root "$TMP/drift" 1.14.0 1.13.2 1.14.0
if out="$(SHERPA_CHECK_ROOT="$TMP/drift" bash "$SCRIPT" --static 2>&1)"; then
  fail "static check passed with Dockerfile 1.13.2 vs Cargo.lock 1.14.0"
fi
grep -q "docker/ci/Dockerfile bakes 1.13.2, Cargo.lock has sherpa-onnx-sys 1.14.0" <<<"$out" || fail "static drift message: $out"

# 3. Musl script default behind Cargo.lock.
make_root "$TMP/musl" 1.14.0 1.14.0 1.13.2
if out="$(SHERPA_CHECK_ROOT="$TMP/musl" bash "$SCRIPT" --static 2>&1)"; then
  fail "static check passed with musl default 1.13.2 vs Cargo.lock 1.14.0"
fi
grep -q "build-sherpa-onnx-musl-libs.sh defaults to 1.13.2" <<<"$out" || fail "musl drift message: $out"

# 4. --runtime --require without SHERPA_ONNX_ARCHIVE_DIR: named error, not a silent pass.
make_root "$TMP/rt" 1.13.2 1.13.2 1.13.2
if out="$(env -u SHERPA_ONNX_ARCHIVE_DIR SHERPA_CHECK_ROOT="$TMP/rt" bash "$SCRIPT" --runtime --require 2>&1)"; then
  fail "runtime check passed without SHERPA_ONNX_ARCHIVE_DIR"
fi
grep -q "SHERPA_ONNX_ARCHIVE_DIR is not set" <<<"$out" || fail "unset dir message: $out"

# 5. Archive dir present but the Cargo.lock version's archive is missing.
mkdir -p "$TMP/archives"
if out="$(SHERPA_CHECK_ROOT="$TMP/rt" SHERPA_ONNX_ARCHIVE_DIR="$TMP/archives" bash "$SCRIPT" --runtime --require 2>&1)"; then
  fail "runtime check passed with an empty archive dir"
fi
grep -q "sherpa-onnx archive missing: $TMP/archives/sherpa-onnx-v1.13.2-linux-x64-static-lib.tar.bz2" <<<"$out" ||
  fail "missing archive message: $out"

# 6. Archive for the Cargo.lock version present: pass.
printf 'x' >"$TMP/archives/sherpa-onnx-v1.13.2-linux-x64-static-lib.tar.bz2"
SHERPA_CHECK_ROOT="$TMP/rt" SHERPA_ONNX_ARCHIVE_DIR="$TMP/archives" bash "$SCRIPT" --runtime --require >/dev/null ||
  fail "runtime check failed with the archive present"

# 7. Without --require outside GitHub Actions: no-op skip.
out="$(env -u SHERPA_ONNX_ARCHIVE_DIR GITHUB_ACTIONS= SHERPA_CHECK_ROOT="$TMP/rt" bash "$SCRIPT" --runtime 2>&1)" ||
  fail "runtime check without --require failed outside GitHub Actions: $out"
grep -q "runtime check skipped" <<<"$out" || fail "skip message: $out"

echo "check-sherpa-onnx-archive.test.sh: OK"
