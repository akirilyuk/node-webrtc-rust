#!/usr/bin/env bash
# Optional local mirror of the GitHub Actions PR **Test** job using Docker:
#   ci-test container + run-pr-integration.sh
#
# CI runs the same script inside ghcr.io/<owner>/node-webrtc-rust/ci-test (ci-build + coturn +
# Sherpa models). run-pr-integration.sh starts turnserver itself (scripts/ci/turnserver-ci.sh)
# and builds the debug linux-gnu .node when packages/bindings has none, so this wrapper only
# mounts the repo and sets TURN_AVAILABLE=1 / CI=true.
#
# Prefer the host path instead:
#   npm run build:native && npm run ci:verify:pr-full
#
# Use this script only when debugging ci-test container differences.
#
# Usage (from repo root):
#   bash scripts/ci/run-pr-test-job-docker.sh
#   bash scripts/ci/run-pr-test-job-docker.sh --quality   # quality job first, then test
#
# Env:
#   CI_IMAGE_LOCAL              docker image (default ghcr.io/akirilyuk/node-webrtc-rust/ci-build:test)
#   CI_STEP_LOG_TS=1            timestamp each ci-step line
#   CI_PR_TEST_SKIP_COTURN=1    do not set TURN_AVAILABLE (no TURN — not full parity)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

IMAGE="${CI_IMAGE_LOCAL:-ghcr.io/akirilyuk/node-webrtc-rust/ci-build:test}"
RUN_QUALITY=false

if [[ "${1:-}" == "--quality" ]]; then
  RUN_QUALITY=true
fi

echo "==> PR Test job (local Docker parity)"
echo "    image: $IMAGE"
echo "    workspace: $ROOT"
echo "    logs: [ci-step] START/OK/FAIL — set CI_STEP_LOG_TS=1 for UTC timestamps"
echo "    Sherpa E2E: [topology] signaling/agent/user + [speech] per role on stderr"
echo ""

echo "==> Pulling CI image: $IMAGE"
docker pull "$IMAGE"

run_in_ci_container() {
  local script="$1"
  # Host node_modules (e.g. darwin esbuild) breaks when bind-mounted into linux ci-test.
  local pre="bash scripts/ci/npm-ci-workspace.sh && "
  local turn_env=()
  if [[ "${CI_PR_TEST_SKIP_COTURN:-}" == "1" ]]; then
    echo "==> Running in ci-test (TURN skipped — not full CI parity)"
  else
    echo "==> Running in ci-test (coturn started in-container by run-pr-integration.sh)"
    turn_env=(-e TURN_AVAILABLE=1)
  fi

  docker run --rm \
    -v "$ROOT:/workspace" \
    -w /workspace \
    "${turn_env[@]+"${turn_env[@]}"}" \
    -e CI=true \
    -e CI_STEP_LOG_TS="${CI_STEP_LOG_TS:-}" \
    -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
    -e OPUS_STATIC=1 \
    "$IMAGE" \
    bash -lc "${pre}bash $script"
}

if $RUN_QUALITY; then
  echo ""
  echo "==> Phase A: PR quality job (host scripts in container)"
  run_in_ci_container scripts/ci/run-pr-quality.sh
fi

echo ""
echo "==> Phase B: PR integration job (cargo + npm test + Sherpa E2E)"
run_in_ci_container scripts/ci/run-pr-integration.sh

echo ""
echo "==> PR Test job (local Docker) OK"
