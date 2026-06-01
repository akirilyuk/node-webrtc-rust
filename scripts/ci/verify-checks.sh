#!/usr/bin/env bash
# Mirror the PR test job checks locally (format, lint, typecheck, cargo test, npm test).
# Run on the host — no Docker required. Run `npm run build:native` first if no .node exists.
# Optional: CI_VERIFY_CHECKS_IN_DOCKER=1 (or npm run ci:verify:checks:docker) for ci-build container parity.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${CI_IMAGE_LOCAL:-node-webrtc-rust-ci-build:local}"
USE_DOCKER="${CI_VERIFY_CHECKS_IN_DOCKER:-}"

run() {
  if [[ -n "$USE_DOCKER" ]]; then
    docker run --rm \
      -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
      -e OPUS_STATIC=1 \
      -v "$ROOT:/workspace" \
      -w /workspace \
      "$IMAGE" \
      bash -lc "$1"
  else
    bash -lc "cd \"$ROOT\" && $1"
  fi
}

if [[ -n "$USE_DOCKER" ]]; then
  echo "==> Using CI Docker image for checks: $IMAGE"
  docker build -t "$IMAGE" "$ROOT/docker/ci" >/dev/null
fi

echo "==> npm ci"
run "bash scripts/ci/npm-ci-workspace.sh"

echo "==> format:check"
run "npm run format:check"

echo "==> lint"
run "npm run lint"

echo "==> typecheck (sdk + signaling sources)"
run "npx tsc --noEmit -p scripts/ci/tsconfig.typecheck.json"

echo "==> build-ts-workspace (PR build-ts + release publish path)"
run "bash scripts/ci/build-ts-workspace.sh"

echo "==> release publish TS parity (npm ci --ignore-scripts + version bump)"
run "bash scripts/ci/verify-release-publish-ts.sh"

echo "==> restore workspace install after release publish simulation"
run "bash scripts/ci/npm-ci-workspace.sh"
run "npm run build:native"

echo "==> cargo test (core, mixer, conference, speech)"
run "cargo test -p node-webrtc-rust-core --lib"
run "cargo test -p node-webrtc-rust-core --test peer_connection_test -- --test-threads=1"
run "cargo test -p node-webrtc-rust-mixer"
run "cargo test -p node-webrtc-rust-conference"
run "cargo test -p node-webrtc-rust-speech"

echo "==> npm test"
run "npm test"

echo "==> Sherpa example typecheck + semantic barge-in E2E"
run "bash scripts/ci/run-sherpa-example-ci.sh all"

echo "==> PR checks OK"
