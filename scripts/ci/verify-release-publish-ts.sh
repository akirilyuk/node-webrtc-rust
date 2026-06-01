#!/usr/bin/env bash
# Mirror release.yml publish job "Build TypeScript packages" preconditions.
#
# Remote publish runs:
#   npm ci --ignore-scripts
#   (napi artifacts + set-release-deps version bump)
#   bash scripts/ci/build-ts-workspace.sh
#
# npm ci installs a registry copy of @node-webrtc-rust/bindings under
# packages/sdk/node_modules/ (stale vs workspace index.d.ts). Release must
# still compile via build-ts-workspace.sh — same failure mode as run 26580426604.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${CI_IMAGE_LOCAL:-node-webrtc-rust-ci-build:local}"
USE_DOCKER="${CI_VERIFY_RELEASE_TS_IN_DOCKER:-}"
VERIFY_VERSION="${VERIFY_RELEASE_VERSION:-9.9.9-verify}"

run_inner() {
  local verify_version="$1"
  local root="$2"

  cd "$root"

  restore_versions() {
    git restore --source=HEAD --staged --worktree \
      packages/bindings/package.json \
      packages/bindings/npm/darwin-arm64/package.json \
      packages/bindings/npm/darwin-x64/package.json \
      packages/bindings/npm/linux-arm64-gnu/package.json \
      packages/bindings/npm/linux-x64-gnu/package.json \
      packages/bindings/npm/linux-x64-musl/package.json \
      packages/bindings/npm/win32-x64-msvc/package.json \
      packages/sdk/package.json \
      packages/signaling/package.json \
      packages/helpers/package.json \
      2>/dev/null || true
  }
  trap restore_versions EXIT

  echo "==> clean install (release publish: npm ci --ignore-scripts)"
  rm -rf node_modules
  find packages -maxdepth 2 -name node_modules -type d -exec rm -rf {} + 2>/dev/null || true
  rm -rf packages/sdk/dist packages/signaling/dist packages/helpers/dist

  npm ci --ignore-scripts

  if [[ ! -f packages/sdk/node_modules/@node-webrtc-rust/bindings/index.d.ts ]]; then
    echo "WARN: expected nested registry bindings under packages/sdk/node_modules — regression signal weakened" >&2
  else
    echo "==> nested registry bindings present (release publish layout OK)"
  fi

  echo "==> simulate release version bump ($verify_version)"
  (
    cd packages/bindings
    npm version "$verify_version" --no-git-tag-version --allow-same-version >/dev/null
  )
  bash scripts/ci/set-release-deps.sh "$verify_version"

  echo "==> build-ts-workspace (release publish compile)"
  bash scripts/ci/build-ts-workspace.sh

  echo "==> Release publish TypeScript path OK"
}

if [[ -n "$USE_DOCKER" ]]; then
  echo "==> Using CI Docker image: $IMAGE"
  docker build -t "$IMAGE" "$ROOT/docker/ci" >/dev/null
  docker run --rm \
    -e VERIFY_RELEASE_VERSION="$VERIFY_VERSION" \
    -v "$ROOT:/workspace" \
    -w /workspace \
    "$IMAGE" \
    bash -lc "bash scripts/ci/verify-release-publish-ts.sh"
  exit 0
fi

run_inner "$VERIFY_VERSION" "$ROOT"
