#!/usr/bin/env bash
# Snapshot / restore packages/bindings NAPI JS+TS surface around `napi build`.
#
# `npx napi build` rewrites index.js (and sometimes index.d.ts). Manifest
# napi_surface_digest and verify-native-binding-surface must use the published
# (pre-build / checkout) bytes — the same ones assemble validates against HEAD.
#
# Prefer filesystem snapshot over `git checkout`: linux compile jobs run in
# Docker containers where `.git` is often unavailable
# (`git rev-parse --is-inside-work-tree` fails).
#
# Usage:
#   bash scripts/ci/bindings-napi-surface.sh snapshot
#   bash scripts/ci/bindings-napi-surface.sh restore
#
# Not part of the fingerprinted compile recipe (build-native-addon.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cmd="${1:-}"
SNAPSHOT_DIR="${NAPI_SURFACE_SNAPSHOT_DIR:-$ROOT/.napi-surface-snapshot}"
SURFACE=(
  packages/bindings/index.js
  packages/bindings/index.d.ts
)

snapshot() {
  mkdir -p "$SNAPSHOT_DIR"
  local rel
  for rel in "${SURFACE[@]}"; do
    if [[ ! -f "$rel" ]]; then
      echo "bindings-napi-surface snapshot: missing $rel" >&2
      exit 1
    fi
    cp "$rel" "$SNAPSHOT_DIR/$(basename "$rel")"
  done
  echo "ok: snapped napi surface to $SNAPSHOT_DIR"
}

restore() {
  local rel base snapped
  local missing=0
  for rel in "${SURFACE[@]}"; do
    base="$(basename "$rel")"
    snapped="$SNAPSHOT_DIR/$base"
    if [[ ! -f "$snapped" ]]; then
      missing=1
      break
    fi
  done

  if [[ "$missing" -eq 0 ]]; then
    local changed=0
    for rel in "${SURFACE[@]}"; do
      base="$(basename "$rel")"
      snapped="$SNAPSHOT_DIR/$base"
      if ! cmp -s "$snapped" "$rel"; then
        changed=1
        break
      fi
    done
    if [[ "$changed" -eq 0 ]]; then
      echo "ok: bindings napi surface already matches snapshot"
      return 0
    fi
    echo "notice: napi build drifted surface; restoring from snapshot $SNAPSHOT_DIR"
    for rel in "${SURFACE[@]}"; do
      base="$(basename "$rel")"
      cp "$SNAPSHOT_DIR/$base" "$rel"
    done
    echo "ok: restored ${SURFACE[*]} from snapshot"
    return 0
  fi

  # Fallback for local/dev hosts when snapshot was not taken.
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if git diff --quiet -- "${SURFACE[@]}"; then
      echo "ok: bindings napi surface already matches HEAD (no snapshot)"
      return 0
    fi
    echo "notice: no snapshot; restoring napi surface from git HEAD"
    git --no-pager diff --stat -- "${SURFACE[@]}" || true
    git checkout HEAD -- "${SURFACE[@]}"
    echo "ok: restored ${SURFACE[*]} from HEAD"
    return 0
  fi

  echo "bindings-napi-surface restore: no snapshot at $SNAPSHOT_DIR and not a git work tree" >&2
  echo "Run 'bindings-napi-surface.sh snapshot' before napi build in container jobs." >&2
  exit 1
}

case "$cmd" in
  snapshot) snapshot ;;
  restore) restore ;;
  *)
    echo "usage: $0 snapshot|restore" >&2
    exit 2
    ;;
esac
