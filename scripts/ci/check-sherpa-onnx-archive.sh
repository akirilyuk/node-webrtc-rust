#!/usr/bin/env bash
# Guard: cargo builds in CI never download the sherpa-onnx prebuilt archive from GitHub releases.
#
# sherpa-onnx-sys's build.rs downloads sherpa-onnx-v<ver>-linux-x64-static-lib.tar.bz2 at build
# time unless SHERPA_ONNX_LIB_DIR or SHERPA_ONNX_ARCHIVE_DIR is set. On 2026-11-09 GitHub answered
# that URL with 504 and every PR's rust lint failed. docker/ci/Dockerfile bakes the archive into
# /opt/sherpa-onnx-archives (sha256-checked) and exports SHERPA_ONNX_ARCHIVE_DIR.
#
# Usage:
#   check-sherpa-onnx-archive.sh --static
#       The sherpa-onnx version baked into docker/ci/Dockerfile and the musl build script default
#       equal sherpa-onnx-sys in Cargo.lock (the single source). Runs anywhere.
#   check-sherpa-onnx-archive.sh --runtime [--require]
#       SHERPA_ONNX_ARCHIVE_DIR holds the linux-x64 static archive for the Cargo.lock version.
#       Without --require it only runs in a GitHub Actions linux x86_64 glibc job (the ci-build
#       image) and is a no-op elsewhere (macOS dev, Alpine/musl, arm64 hosts).
#
# Env (tests): SHERPA_CHECK_ROOT overrides the repo root.
set -euo pipefail

ROOT="${SHERPA_CHECK_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"

die() {
  echo "check-sherpa-onnx-archive: $*" >&2
  exit 1
}

lock_version() {
  local v
  v="$(awk '/^name = "sherpa-onnx-sys"$/ { getline; gsub(/version = |"/, ""); print; exit }' "$ROOT/Cargo.lock")"
  [[ -n "$v" ]] || die "sherpa-onnx-sys not found in $ROOT/Cargo.lock"
  printf '%s' "$v"
}

check_static() {
  local want dockerfile musl_script baked musl
  want="$(lock_version)"
  dockerfile="$ROOT/docker/ci/Dockerfile"
  musl_script="$ROOT/scripts/ci/build-sherpa-onnx-musl-libs.sh"
  baked="$(sed -n 's/^ARG SHERPA_ONNX_VERSION=//p' "$dockerfile" | head -1)"
  [[ -n "$baked" ]] || die "no 'ARG SHERPA_ONNX_VERSION=' in $dockerfile"
  [[ "$baked" == "$want" ]] ||
    die "sherpa-onnx version mismatch: docker/ci/Dockerfile bakes $baked, Cargo.lock has sherpa-onnx-sys $want. Update ARG SHERPA_ONNX_VERSION and ARG SHERPA_ONNX_LINUX_X64_STATIC_SHA256, then rebuild the ci-build image (ci-image.yml)."
  musl="$(sed -n 's/^SHERPA_VERSION="\${SHERPA_ONNX_VERSION:-\([^}]*\)}"$/\1/p' "$musl_script" | head -1)"
  [[ -n "$musl" ]] || die "no SHERPA_VERSION default in $musl_script"
  [[ "$musl" == "$want" ]] ||
    die "sherpa-onnx version mismatch: build-sherpa-onnx-musl-libs.sh defaults to $musl, Cargo.lock has sherpa-onnx-sys $want."
  echo "check-sherpa-onnx-archive: static OK (sherpa-onnx-sys $want)"
}

runtime_applies() {
  [[ "${GITHUB_ACTIONS:-}" == "true" ]] || return 1
  [[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]] || return 1
  [[ ! -f /etc/alpine-release ]] || return 1
  return 0
}

check_runtime() {
  local require="$1" want name dir
  if [[ "$require" != "1" ]] && ! runtime_applies; then
    echo "check-sherpa-onnx-archive: runtime check skipped (not a GitHub Actions linux x86_64 glibc job)"
    return 0
  fi
  want="$(lock_version)"
  name="sherpa-onnx-v${want}-linux-x64-static-lib.tar.bz2"
  dir="${SHERPA_ONNX_ARCHIVE_DIR:-}"
  [[ -n "$dir" ]] ||
    die "SHERPA_ONNX_ARCHIVE_DIR is not set: cargo would download $name from GitHub releases at build time. Run this job in the ci-build image (docker/ci/Dockerfile bakes the archive)."
  [[ -s "$dir/$name" ]] ||
    die "sherpa-onnx archive missing: $dir/$name (Cargo.lock sherpa-onnx-sys $want). The ci-build image is older than Cargo.lock; bump ARG SHERPA_ONNX_VERSION in docker/ci/Dockerfile and rebuild the image (ci-image.yml)."
  echo "check-sherpa-onnx-archive: runtime OK ($dir/$name)"
}

mode="${1:-}"
case "$mode" in
  --static) check_static ;;
  --runtime)
    require=0
    [[ "${2:-}" == "--require" ]] && require=1
    check_runtime "$require"
    ;;
  *)
    echo "usage: $0 --static | --runtime [--require]" >&2
    exit 2
    ;;
esac
