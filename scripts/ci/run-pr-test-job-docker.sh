#!/usr/bin/env bash
# Optional local mirror of the GitHub Actions PR **Test** job using Docker:
#   coturn sidecar + ci-build container + run-pr-integration.sh
#
# Prefer the host path instead:
#   npm run build:native && npm run ci:verify:pr-full
#
# Use this script only when debugging remote ci-build container / coturn differences.
#
# Usage (from repo root):
#   bash scripts/ci/run-pr-test-job-docker.sh
#   bash scripts/ci/run-pr-test-job-docker.sh --quality   # quality job first, then test
#
# Env:
#   CI_IMAGE_LOCAL              docker image tag (default node-webrtc-rust-ci-build:local)
#   CI_STEP_LOG_TS=1            timestamp each ci-step line
#   CI_PR_TEST_SKIP_COTURN=1    run in ci-build only (no TURN — not full parity)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

IMAGE="${CI_IMAGE_LOCAL:-node-webrtc-rust-ci-build:local}"
COTURN_NAME="coturn-pr-local-$$"
RUN_QUALITY=false

if [[ "${1:-}" == "--quality" ]]; then
  RUN_QUALITY=true
fi

cleanup() {
  docker rm -f "$COTURN_NAME" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> PR Test job (local Docker parity)"
echo "    image: $IMAGE"
echo "    workspace: $ROOT"
echo "    logs: [ci-step] START/OK/FAIL — set CI_STEP_LOG_TS=1 for UTC timestamps"
echo "    Sherpa E2E: [topology] signaling/agent/user + [speech] per role on stderr"
echo ""

echo "==> Building CI Docker image (if needed): $IMAGE"
docker build -t "$IMAGE" "$ROOT/docker/ci"

ensure_linux_gnu_node() {
  shopt -s nullglob
  local gnu=(packages/bindings/*linux-gnu*.node packages/bindings/node-webrtc-rust.linux-x64-gnu.node)
  if [[ ${#gnu[@]} -gt 0 ]]; then
    echo "==> Native binding: using $(basename "${gnu[0]}")"
    return 0
  fi
  echo "==> No linux-gnu .node — compiling debug x86_64-unknown-linux-gnu in ci-build"
  docker run --rm \
    -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
    -e OPUS_STATIC=1 \
    -v "$ROOT:/workspace" \
    -w /workspace/packages/bindings \
    "$IMAGE" \
    bash -lc 'npm ci --ignore-scripts && npx napi build --target x86_64-unknown-linux-gnu && npm run copy:local-node'
  shopt -s nullglob
  gnu=(packages/bindings/*linux-gnu*.node packages/bindings/node-webrtc-rust.linux-x64-gnu.node)
  if [[ ${#gnu[@]} -eq 0 ]]; then
    echo "linux-gnu .node still missing after container compile." >&2
    exit 1
  fi
  echo "==> Native binding ready: $(basename "${gnu[0]}")"
}

ensure_linux_gnu_node

run_in_ci_container() {
  local script="$1"
  # Host node_modules (e.g. darwin esbuild) breaks when bind-mounted into linux ci-build.
  local pre="bash scripts/ci/npm-ci-workspace.sh && "
  if [[ "${CI_PR_TEST_SKIP_COTURN:-}" == "1" ]]; then
    echo "==> Running in ci-build (TURN skipped — not full CI parity)"
    docker run --rm \
      -v "$ROOT:/workspace" \
      -w /workspace \
      -e CI_STEP_LOG_TS="${CI_STEP_LOG_TS:-}" \
      -e CMAKE_POLICY_VERSION_MINIMUM=3.5 \
      -e OPUS_STATIC=1 \
      "$IMAGE" \
      bash -lc "${pre}bash $script"
    return
  fi

  echo "==> Starting coturn sidecar ($COTURN_NAME) — same as GitHub Actions Test job"
  docker rm -f "$COTURN_NAME" 2>/dev/null || true
  docker run -d --name "$COTURN_NAME" coturn/coturn:latest \
    turnserver -n -L 0.0.0.0:3478 -a \
    -u testuser:testpass -r test.local \
    --min-port 49152 --max-port 65535 \
    --no-tls --no-dtls --log-file=stdout -v \
    --allow-loopback-peers \
    --external-ip=127.0.0.1/127.0.0.1

  docker run --rm --network "container:${COTURN_NAME}" \
    -v "$ROOT:/workspace" \
    -w /workspace \
    -e TURN_AVAILABLE=1 \
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
