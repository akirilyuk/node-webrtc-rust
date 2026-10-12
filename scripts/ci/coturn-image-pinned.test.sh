#!/usr/bin/env bash
# Guard: CI must not start coturn from :latest or any image without a sha256 digest.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

files=()
while IFS= read -r f; do files+=("$f"); done < <(
  grep -rlE 'coturn/coturn' .github scripts/ci --include='*.yml' --include='*.yaml' --include='*.sh' 2>/dev/null \
    | grep -v 'coturn-image-pinned.test.sh' || true
)

if [[ ${#files[@]} -eq 0 ]]; then
  echo "FAIL: no CI file references coturn/coturn (guard is stale)" >&2
  exit 1
fi

fail=0
for f in "${files[@]}"; do
  # Image refs: coturn/coturn followed by :tag or @digest, outside prose comments.
  while IFS= read -r line; do
    case "$line" in
      *'coturn/coturn:latest'*)
        echo "FAIL: $f references coturn/coturn:latest" >&2; fail=1 ;;
      *'coturn/coturn:'*'@sha256:'*) ;;
      *'docker run'*'coturn/coturn'*)
        echo "FAIL: $f runs an unpinned coturn image (needs tag@sha256:...): $line" >&2; fail=1 ;;
    esac
  done < <(grep -E 'coturn/coturn' "$f" | grep -vE '^\s*#|^name:|^description:|\(coturn/coturn|coturn/coturn \+|coturn/coturn\)' || true)
done
[[ "$fail" -eq 0 ]] || exit 1
echo "OK: coturn image is digest-pinned in ${#files[@]} file(s)"
