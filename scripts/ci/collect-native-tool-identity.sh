#!/usr/bin/env bash
# Emit export lines for declared + resolved native tool identities.
#
#   eval "$(bash scripts/ci/collect-native-tool-identity.sh --target TRIPLE)"
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

target=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) target="${2:-}"; shift 2 ;;
    --export) shift ;; # backwards compatible no-op
    *)
      echo "collect-native-tool-identity: unknown arg: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$target" ]]; then
  echo "collect-native-tool-identity: --target required" >&2
  exit 1
fi

TARGET="$target" python3 - <<'PY'
import json, os, shlex, subprocess, sys
from pathlib import Path

sys.path.insert(0, str(Path("scripts/ci").resolve()))
import native_build_contract as nbc

target = os.environ["TARGET"]
root = nbc.repo_root()
declared = nbc.declared_tool_identity(target, root)
env_map = {
    "rustc": "NATIVE_RUSTC_IDENTITY",
    "cargo": "NATIVE_CARGO_IDENTITY",
    "node": "NATIVE_NODE_IDENTITY",
    "image": "NATIVE_IMAGE_DIGEST",
    "runner": "NATIVE_RUNNER_LABEL",
    "host_sdk": "NATIVE_HOST_SDK_IDENTITY",
    "zig": "NATIVE_ZIG_IDENTITY",
    "napi_cli": "NATIVE_NAPI_CLI_IDENTITY",
}
for k, env in env_map.items():
    print(f"export {env}={shlex.quote(declared[k])}")
print("export NATIVE_TOOL_MODE=declared")

def run(cmd):
    try:
        return subprocess.check_output(cmd, text=True, stderr=subprocess.DEVNULL).strip()
    except (FileNotFoundError, subprocess.CalledProcessError):
        return ""

rustc = run(["rustc", "-Vv"]).replace("\n", "|")
cargo = run(["cargo", "-V"])
node = run(["node", "-v"])
if rustc:
    print(f"export NATIVE_RESOLVED_RUSTC={shlex.quote(rustc)}")
if cargo:
    print(f"export NATIVE_RESOLVED_CARGO={shlex.quote(cargo)}")
if node:
    print(f"export NATIVE_RESOLVED_NODE={shlex.quote(node)}")

os_release = Path("/etc/os-release")
if os_release.is_file():
    pretty = ""
    for line in os_release.read_text(encoding="utf-8").splitlines():
        if line.startswith("PRETTY_NAME="):
            pretty = line.split("=", 1)[1].strip().strip('"')
    if pretty:
        print(f"export NATIVE_RESOLVED_HOST_SDK={shlex.quote(pretty)}")

runner = f"{os.environ.get('RUNNER_NAME') or os.environ.get('RUNNER_OS') or 'local'}/{os.environ.get('RUNNER_ARCH') or 'unknown'}"
print(f"export NATIVE_RESOLVED_RUNNER={shlex.quote(runner)}")
if os.environ.get("CI_IMAGE"):
    print(f"export NATIVE_RESOLVED_IMAGE={shlex.quote(os.environ['CI_IMAGE'])}")

zig = run(["zig", "version"])
if zig:
    print(f"export NATIVE_RESOLVED_ZIG={shlex.quote(zig)}")

napi_pkg = Path("packages/bindings/node_modules/@napi-rs/cli/package.json")
if napi_pkg.is_file():
    ver = json.loads(napi_pkg.read_text(encoding="utf-8")).get("version")
    if ver:
        print(f"export NATIVE_RESOLVED_NAPI_CLI={shlex.quote(str(ver))}")
PY

# Musl builds need the lib dir marker for fingerprint parity with CI.
if [[ "$target" == *musl* ]]; then
  if [[ -n "${SHERPA_ONNX_LIB_DIR:-}" ]]; then
    echo "export NATIVE_SHERPA_ONNX_LIB_DIR=$(printf '%q' "$SHERPA_ONNX_LIB_DIR")"
  else
    echo "export NATIVE_SHERPA_ONNX_LIB_DIR=/opt/sherpa-musl/lib"
  fi
fi
