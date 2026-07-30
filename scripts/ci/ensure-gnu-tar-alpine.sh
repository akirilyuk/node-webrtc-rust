#!/usr/bin/env bash
# actions/cache and Swatinem/rust-cache require GNU tar (--posix / -P).
# Alpine images ship BusyBox tar at /bin/tar, which rejects those options and
# silently breaks native + Cargo cache restore/save on musl jobs.
set -euo pipefail

if [[ ! -f /etc/os-release ]] || ! grep -qi '^ID=alpine' /etc/os-release; then
  exit 0
fi

is_gnu_tar() {
  local bin="${1:-tar}"
  "$bin" --version 2>/dev/null | head -n 1 | grep -qi 'gnu tar'
}

if is_gnu_tar tar && is_gnu_tar /bin/tar; then
  echo "GNU tar already available: $(tar --version | head -n 1)"
  exit 0
fi

if ! command -v apk >/dev/null 2>&1; then
  echo "ensure-gnu-tar-alpine: apk not found; cannot install GNU tar" >&2
  exit 1
fi

apk add --no-cache tar gzip

# Alpine's tar package installs GNU tar; BusyBox may still own /bin/tar.
gnu_bin=""
for candidate in /usr/bin/tar /bin/tar "$(command -v tar)"; do
  if [[ -n "$candidate" && -x "$candidate" ]] && is_gnu_tar "$candidate"; then
    gnu_bin="$candidate"
    break
  fi
done

if [[ -z "$gnu_bin" ]]; then
  echo "ensure-gnu-tar-alpine: GNU tar missing after apk add tar" >&2
  tar --version 2>&1 | head -n 5 >&2 || true
  exit 1
fi

if ! is_gnu_tar /bin/tar; then
  ln -sfn "$gnu_bin" /bin/tar
fi

if ! is_gnu_tar /bin/tar; then
  echo "ensure-gnu-tar-alpine: /bin/tar is still not GNU tar" >&2
  /bin/tar --version 2>&1 | head -n 5 >&2 || true
  exit 1
fi

echo "GNU tar ready for Actions cache: $(/bin/tar --version | head -n 1)"
