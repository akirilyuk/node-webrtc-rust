#!/usr/bin/env bash
# Plan which release native targets need compile vs reuse GitHub Actions cache.
#
# Uses exact cache keys (native-v2-{profile}-{target}-{hash}) — same as native-binding-cache.
# Outputs multiline GITHUB_OUTPUT for workflow matrices.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

PROFILE="${PLAN_NATIVE_PROFILE:-release}"
REPO="${GITHUB_REPOSITORY:-}"

if [[ -z "$REPO" ]]; then
  echo "GITHUB_REPOSITORY is required" >&2
  exit 1
fi

HASH="$(bash scripts/ci/native-binding-cache-key.sh)"

cache_exists() {
  local target="$1"
  local key="native-v2-${PROFILE}-${target}-${HASH}"
  local count
  count="$(
    gh api -H "Accept: application/vnd.github+json" \
      "/repos/${REPO}/actions/caches?key=${key}" \
      --jq '.total_count // 0' 2>/dev/null || echo 0
  )"
  [[ "${count:-0}" -gt 0 ]]
}

need_gnu=false
need_musl=false
need_arm64=false
cached_targets=()

while IFS= read -r target; do
  [[ -z "$target" ]] && continue
  if cache_exists "$target"; then
    cached_targets+=("$target")
    echo "  cache hit: $target"
  else
    echo "  need build: $target"
    case "$target" in
      x86_64-unknown-linux-gnu) need_gnu=true ;;
      x86_64-unknown-linux-musl) need_musl=true ;;
      aarch64-unknown-linux-gnu) need_arm64=true ;;
    esac
  fi
done < <(bash scripts/ci/list-release-targets.sh)

linux_x64_matrix=()
if [[ "$need_gnu" == true ]]; then
  linux_x64_matrix+=('{"target":"x86_64-unknown-linux-gnu","zig":"false"}')
fi

build_linux_musl=false
if [[ "$need_musl" == true ]]; then
  build_linux_musl=true
fi

host_matrix=()
host_entry() {
  local target="$1" os="$2" args="$3"
  if ! cache_exists "$target"; then
    host_matrix+=("{\"target\":\"${target}\",\"os\":\"${os}\",\"build-args\":\"${args}\"}")
  fi
}

host_entry x86_64-apple-darwin macos-latest '--target x86_64-apple-darwin'
host_entry aarch64-apple-darwin macos-latest '--target aarch64-apple-darwin'
host_entry x86_64-pc-windows-msvc windows-latest '--target x86_64-pc-windows-msvc'

linux_x64_json='[]'
if [[ ${#linux_x64_matrix[@]} -gt 0 ]]; then
  linux_x64_json="[$(IFS=,; echo "${linux_x64_matrix[*]}")]"
fi

host_json='[]'
if [[ ${#host_matrix[@]} -gt 0 ]]; then
  host_json="[$(IFS=,; echo "${host_matrix[*]}")]"
fi

cached_json='[]'
cached_linux_x64=()
cached_linux_arm64=false
cached_host=()

for t in "${cached_targets[@]}"; do
  case "$t" in
    x86_64-unknown-linux-gnu|x86_64-unknown-linux-musl)
      cached_linux_x64+=("\"${t}\"")
      ;;
    aarch64-unknown-linux-gnu)
      cached_linux_arm64=true
      ;;
    x86_64-apple-darwin|aarch64-apple-darwin|x86_64-pc-windows-msvc)
      cached_host+=("\"${t}\"")
      ;;
  esac
done

if [[ ${#cached_targets[@]} -gt 0 ]]; then
  quoted=()
  for t in "${cached_targets[@]}"; do
    quoted+=("\"${t}\"")
  done
  cached_json="[$(IFS=,; echo "${quoted[*]}")]"
fi

cached_linux_x64_json='[]'
if [[ ${#cached_linux_x64[@]} -gt 0 ]]; then
  cached_linux_x64_json="[$(IFS=,; echo "${cached_linux_x64[*]}")]"
fi

cached_host_json='[]'
if [[ ${#cached_host[@]} -gt 0 ]]; then
  cached_host_json="[$(IFS=,; echo "${cached_host[*]}")]"
fi

all_cached=false
if [[ ${#cached_targets[@]} -eq 6 ]]; then
  all_cached=true
fi

{
  echo "native_hash=${HASH}"
  echo "linux_x64_matrix<<EOF"
  echo "$linux_x64_json"
  echo "EOF"
  echo "build_linux_arm64=${need_arm64}"
  echo "build_linux_musl=${build_linux_musl}"
  echo "host_matrix<<EOF"
  echo "$host_json"
  echo "EOF"
  echo "cached_targets<<EOF"
  echo "$cached_json"
  echo "EOF"
  echo "cached_linux_x64<<EOF"
  echo "$cached_linux_x64_json"
  echo "EOF"
  echo "cached_linux_arm64=${cached_linux_arm64}"
  echo "cached_host<<EOF"
  echo "$cached_host_json"
  echo "EOF"
  echo "all_cached=${all_cached}"
} >> "${GITHUB_OUTPUT:-/dev/stdout}"

echo "==> Native build plan (hash=${HASH})"
echo "    all_cached=${all_cached}"
echo "    linux_x64_matrix=${linux_x64_json}"
echo "    build_linux_arm64=${need_arm64}"
echo "    build_linux_musl=${build_linux_musl}"
echo "    host_matrix=${host_json}"
echo "    cached_targets=${cached_json}"
