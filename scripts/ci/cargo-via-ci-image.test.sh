#!/usr/bin/env bash
# Unit tests for CARGO_VIA_CI_IMAGE docker-wrapped cargo metadata (no real ci-build image).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

python3 - <<'PY'
import os
import sys
from pathlib import Path

sys.path.insert(0, "scripts/ci")
import native_build_contract as nbc

root = Path.cwd().resolve()
target = "x86_64-unknown-linux-gnu"
features = ["otel"]

os.environ.pop("CARGO_VIA_CI_IMAGE", None)
os.environ.pop("CI_IMAGE", None)
os.environ.pop("CI_IMAGE_LOCAL", None)

host_cmd = nbc.build_cargo_metadata_command(root, target, features)
if host_cmd[0] != "cargo" or "docker" in host_cmd:
    raise SystemExit(f"expected host cargo argv, got: {host_cmd!r}")

os.environ["CARGO_VIA_CI_IMAGE"] = "1"
os.environ["CI_IMAGE"] = "ghcr.io/akirilyuk/node-webrtc-rust/ci-build:latest"
docker_cmd = nbc.build_cargo_metadata_command(root, target, features)
if docker_cmd[:3] != ["docker", "run", "--rm"]:
    raise SystemExit(f"expected docker run prefix, got: {docker_cmd[:5]!r}")
root_s = str(root)
vol = f"{root_s}:{root_s}"
if vol not in docker_cmd:
    raise SystemExit("docker command missing workspace volume mount")
if docker_cmd[docker_cmd.index("-w") + 1] != root_s:
    raise SystemExit("docker command missing -w workspace")
if "CARGO_HOME=/usr/local/cargo" not in docker_cmd:
    raise SystemExit("docker command missing CARGO_HOME")
if "RUSTUP_HOME=/usr/local/rustup" not in docker_cmd:
    raise SystemExit("docker command missing RUSTUP_HOME")
image_idx = docker_cmd.index("ghcr.io/akirilyuk/node-webrtc-rust/ci-build:latest")
cargo_tail = docker_cmd[image_idx + 1 :]
expected_tail = nbc.build_cargo_metadata_argv(root, target, features)
if cargo_tail != expected_tail:
    raise SystemExit(f"cargo tail mismatch: {cargo_tail!r} vs {expected_tail!r}")

os.environ.pop("CI_IMAGE", None)
os.environ["CI_IMAGE_LOCAL"] = "ghcr.io/example/ci-build:local"
local_cmd = nbc.build_cargo_metadata_command(root, target, features)
if "ghcr.io/example/ci-build:local" not in local_cmd:
    raise SystemExit("CI_IMAGE_LOCAL fallback not used")

os.environ.pop("CI_IMAGE_LOCAL", None)
try:
    nbc.build_cargo_metadata_command(root, target, features)
except SystemExit as exc:
    if "CI_IMAGE" not in str(exc):
        raise
else:
    raise SystemExit("expected SystemExit when CARGO_VIA_CI_IMAGE set without CI_IMAGE")

for truthy in ("true", "yes", "TRUE"):
    os.environ["CARGO_VIA_CI_IMAGE"] = truthy
    os.environ["CI_IMAGE"] = "ghcr.io/example/ci-build:tag"
    if not nbc.cargo_via_ci_image_enabled():
        raise SystemExit(f"CARGO_VIA_CI_IMAGE={truthy!r} should enable docker cargo")

print("ok: build_cargo_metadata_command docker wrapper")
PY

echo "cargo-via-ci-image.test.sh: all checks passed"
