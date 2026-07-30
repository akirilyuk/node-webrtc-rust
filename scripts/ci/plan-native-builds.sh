#!/usr/bin/env bash
# Plan which release native targets need compile vs per-target Actions cache.
#
# Uses exact cache keys (native-v3-{profile}-{target}-{digest}) — same as
# native-binding-cache. Exact key match via GitHub REST (curl); never trusts
# prefix total_count alone. On API/restore uncertainty → schedule build.
#
# Outputs multiline GITHUB_OUTPUT for workflow matrices.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

PROFILE="${PLAN_NATIVE_PROFILE:-release}"
REPO="${GITHUB_REPOSITORY:-}"
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

if [[ -z "$REPO" ]]; then
  echo "GITHUB_REPOSITORY is required" >&2
  exit 1
fi

export NATIVE_TOOL_MODE="${NATIVE_TOOL_MODE:-declared}"

cache_key_for() {
  local target="$1"
  eval "$(bash scripts/ci/collect-native-tool-identity.sh --target "$target")"
  python3 scripts/ci/native_build_contract.py cache-key --target "$target" --profile "$PROFILE"
}

exact_cache_exists() {
  local target="$1"
  local key="$2"
  if [[ -z "$TOKEN" ]]; then
    echo "  cache probe skipped (no token): $target → build" >&2
    return 1
  fi
  local enc_key body hits
  enc_key="$(printf '%s' "$key" | python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.stdin.read(), safe=""))')"
  body="$(
    curl -fsSL \
      -H "Authorization: Bearer ${TOKEN}" \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      "https://api.github.com/repos/${REPO}/actions/caches?per_page=100&key=${enc_key}" \
      2>/dev/null || true
  )"
  if [[ -z "$body" ]]; then
    echo "  cache API error: $target → build" >&2
    return 1
  fi
  hits="$(
    CACHE_KEY="$key" python3 -c '
import json, os, sys
try:
    data = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit
key = os.environ["CACHE_KEY"]
n = sum(1 for c in (data.get("actions_caches") or []) if c.get("key") == key)
print(n)
' <<<"$body" 2>/dev/null || echo 0
  )"
  [[ "${hits:-0}" -gt 0 ]]
}

need_gnu=false
need_musl=false
need_arm64=false
cached_targets=()
DIGEST_LINES=""

AGGREGATE="$(
  eval "$(bash scripts/ci/collect-native-tool-identity.sh --target x86_64-unknown-linux-gnu)"
  # Aggregate helper applies per-target declared tools internally when GITHUB_ACTIONS/NATIVE_TOOL_MODE set.
  NATIVE_TOOL_MODE=declared python3 scripts/ci/native_build_contract.py aggregate-digest --profile "$PROFILE"
)"

while IFS= read -r target; do
  [[ -z "$target" ]] && continue
  key="$(cache_key_for "$target")"
  digest="${key##*-}"
  DIGEST_LINES="${DIGEST_LINES}${target}=${digest}"$'\n'
  echo "  key: $target → ${key}"
  if exact_cache_exists "$target" "$key"; then
    cached_targets+=("$target")
    echo "  cache hit (exact): $target"
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
  linux_x64_matrix+=('{"target":"x86_64-unknown-linux-gnu"}')
fi

build_linux_musl=false
if [[ "$need_musl" == true ]]; then
  build_linux_musl=true
fi

is_cached() {
  local target="$1" t
  for t in "${cached_targets[@]+"${cached_targets[@]}"}"; do
    [[ "$t" == "$target" ]] && return 0
  done
  return 1
}

host_matrix=()
host_entry() {
  local target="$1" os="$2"
  if ! is_cached "$target"; then
    host_matrix+=("{\"target\":\"${target}\",\"os\":\"${os}\"}")
  fi
}

host_entry x86_64-apple-darwin macos-latest
host_entry aarch64-apple-darwin macos-latest
host_entry x86_64-pc-windows-msvc windows-latest

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

for t in "${cached_targets[@]+"${cached_targets[@]}"}"; do
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

per_target_json="$(
  DIGEST_LINES="$DIGEST_LINES" python3 - <<'PY'
import json, os
out = {}
for line in os.environ.get("DIGEST_LINES", "").splitlines():
    if not line or "=" not in line:
        continue
    t, d = line.split("=", 1)
    out[t] = d
print(json.dumps(out, sort_keys=True))
PY
)"

rebuilt_json="$(
  CACHED_JSON="$cached_json" python3 - <<'PY'
import json, os, subprocess
from pathlib import Path
root = Path(".")
targets = subprocess.check_output(["bash", "scripts/ci/list-release-targets.sh"], text=True)
all_t = [t.strip() for t in targets.splitlines() if t.strip()]
cached = set(json.loads(os.environ.get("CACHED_JSON") or "[]"))
print(json.dumps([t for t in all_t if t not in cached]))
PY
)"

{
  echo "native_hash=${AGGREGATE}"
  echo "aggregate_digest=${AGGREGATE}"
  echo "per_target_digests<<EOF"
  echo "$per_target_json"
  echo "EOF"
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
  echo "rebuilt_targets<<EOF"
  echo "$rebuilt_json"
  echo "EOF"
} >> "${GITHUB_OUTPUT:-/dev/stdout}"

echo "==> Native build plan (aggregate=${AGGREGATE})"
echo "    all_cached=${all_cached}"
echo "    linux_x64_matrix=${linux_x64_json}"
echo "    build_linux_arm64=${need_arm64}"
echo "    build_linux_musl=${build_linux_musl}"
echo "    host_matrix=${host_json}"
echo "    cached_targets=${cached_json}"
echo "    rebuilt_targets=${rebuilt_json}"

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  SUMMARY_TITLE="${PLAN_SUMMARY_TITLE:-Native build plan}" \
  AGGREGATE_DIGEST="$AGGREGATE" \
  ALL_CACHED="$all_cached" \
  CACHED_TARGETS_JSON="$cached_json" \
  REBUILT_TARGETS_JSON="$rebuilt_json" \
  FALLBACK_REASON="${PLAN_FALLBACK_REASON:-}" \
  PRODUCER_SHA="${NATIVE_PRODUCER_GIT_SHA:-}" \
  PRODUCER_RUN_ID="${NATIVE_PRODUCER_RUN_ID:-}" \
  bash scripts/ci/write-native-ci-summary.sh
fi
