#!/usr/bin/env python3
"""Native build fingerprint + provenance contract (chunk 1 foundation).

Canonical, target-specific digests for reusable .node artifacts.
Workflow wiring that consumes these digests lands in a later chunk.

Subcommands:
  fingerprint          Print native input digest (SHA-256 hex)
  distribution-digest  Print distribution digest (SHA-256 hex)
  contract-json        Print the full native input contract as JSON
  list-local-crates    Print local Cargo crates in the bindings closure
  produce-manifest     Write a per-target provenance manifest (JSON)
  validate-manifest    Validate a provenance manifest against disk / digest
  check-release-targets
                       Verify the six release targets + npm mappings
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from collections import deque
from pathlib import Path
from typing import Any, Iterable

SCHEMA = "node-webrtc-rust.native-artifact-manifest/v1"
BUNDLE_SCHEMA = "node-webrtc-rust.native-main-bundle/v1"
BINDINGS_PACKAGE = "node-webrtc-rust-bindings"
NATIVE_CACHE_KEY_PREFIX = "native-v3"
BUNDLE_ARTIFACT_NAME = "native-main-bundle"
MAIN_WORKFLOW_FILE = "build-main.yml"
MAIN_WORKFLOW_NAME = "Build & Test (main)"

# Rust triple → npm platform package directory / optionalDependency suffix / .node basename
RELEASE_TARGET_MAP: dict[str, dict[str, str]] = {
    "x86_64-unknown-linux-gnu": {
        "npm_dir": "linux-x64-gnu",
        "npm_package": "@node-webrtc-rust/bindings-linux-x64-gnu",
        "node_basename": "node-webrtc-rust.linux-x64-gnu.node",
    },
    "x86_64-unknown-linux-musl": {
        "npm_dir": "linux-x64-musl",
        "npm_package": "@node-webrtc-rust/bindings-linux-x64-musl",
        "node_basename": "node-webrtc-rust.linux-x64-musl.node",
    },
    "aarch64-unknown-linux-gnu": {
        "npm_dir": "linux-arm64-gnu",
        "npm_package": "@node-webrtc-rust/bindings-linux-arm64-gnu",
        "node_basename": "node-webrtc-rust.linux-arm64-gnu.node",
    },
    "x86_64-apple-darwin": {
        "npm_dir": "darwin-x64",
        "npm_package": "@node-webrtc-rust/bindings-darwin-x64",
        "node_basename": "node-webrtc-rust.darwin-x64.node",
    },
    "aarch64-apple-darwin": {
        "npm_dir": "darwin-arm64",
        "npm_package": "@node-webrtc-rust/bindings-darwin-arm64",
        "node_basename": "node-webrtc-rust.darwin-arm64.node",
    },
    "x86_64-pc-windows-msvc": {
        "npm_dir": "win32-x64-msvc",
        "npm_package": "@node-webrtc-rust/bindings-win32-x64-msvc",
        "node_basename": "node-webrtc-rust.win32-x64-msvc.node",
    },
}

MUSL_ONLY_PATHS = (
    "docker/ci/Dockerfile.alpine",
    "scripts/ci/install-alpine-native-toolchain.sh",
    "scripts/ci/build-sherpa-onnx-musl-libs.sh",
)

GNU_LINUX_PATHS = (
    "docker/ci/Dockerfile",
)

# Only the canonical compile recipe and explicit epoch affect compiled bytes.
# Cache, fingerprint, manifest, bundle, resolver, and workflow orchestration
# must not invalidate an otherwise identical Rust binary.
RECIPE_PATHS = (
    "scripts/ci/build-native-addon.sh",
    "scripts/ci/native-cache-epoch",
)

# Distribution-only inputs (not part of native-byte digest).
DISTRIBUTION_SURFACE_PATHS = (
    "packages/bindings/index.js",
    "packages/bindings/index.d.ts",
)


def repo_root() -> Path:
    env = os.environ.get("NATIVE_CONTRACT_ROOT")
    if env:
        return Path(env).resolve()
    return Path(__file__).resolve().parents[2]


def _relpath_or_abs(path: Path, root: Path) -> str:
    try:
        return str(path.resolve().relative_to(root.resolve()))
    except ValueError:
        return str(path)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_text(text: str) -> str:
    return sha256_bytes(text.encode("utf-8"))


def stable_json(obj: Any) -> str:
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def read_cache_epoch(root: Path) -> str:
    env = os.environ.get("NATIVE_CACHE_EPOCH")
    if env is not None and env != "":
        return env.strip()
    path = root / "scripts/ci/native-cache-epoch"
    if not path.is_file():
        raise SystemExit(f"native_build_contract: missing cache epoch file: {path}")
    return path.read_text(encoding="utf-8").strip()


def list_release_targets(root: Path) -> list[str]:
    script = root / "scripts/ci/list-release-targets.sh"
    if not script.is_file():
        raise SystemExit(f"native_build_contract: missing {script}")
    out = subprocess.check_output(["bash", str(script)], cwd=str(root), text=True)
    targets = [line.strip() for line in out.splitlines() if line.strip()]
    if len(targets) != 6:
        raise SystemExit(
            f"native_build_contract: expected 6 release targets from "
            f"list-release-targets.sh, got {len(targets)}: {targets}"
        )
    expected = list(RELEASE_TARGET_MAP.keys())
    if targets != expected:
        raise SystemExit(
            "native_build_contract: list-release-targets.sh order/content mismatch.\n"
            f"  script: {targets}\n"
            f"  map:    {expected}"
        )
    return targets


def profile_features(profile: str) -> list[str]:
    if profile == "release":
        return ["otel"]
    if profile == "debug":
        return []
    raise SystemExit(f"native_build_contract: unsupported profile {profile!r} (use debug|release)")


def _napi_cli_identity_default(root: Path | None = None) -> str:
    explicit = os.environ.get("NATIVE_NAPI_CLI_IDENTITY")
    if explicit:
        return explicit
    try:
        base = root or repo_root()
        lock = json.loads((base / "package-lock.json").read_text(encoding="utf-8"))
        ver = ((lock.get("packages") or {}).get("node_modules/@napi-rs/cli") or {}).get(
            "version"
        )
        if ver:
            return str(ver)
    except (OSError, json.JSONDecodeError, SystemExit):
        pass
    return "unresolved"


def _ci_image_content_ref(root: Path, repo: str, kind: str) -> str:
    """Stable image contract from Dockerfile/script bytes — not mutable :latest.

    Prefer an explicit registry digest (CI_IMAGE_DIGEST / CI_IMAGE_ALPINE_DIGEST)
    when the workflow recorded one after push; otherwise key by content hash of
    every file that is COPY'd/executed into that image.
    """
    if kind == "alpine":
        name = "ci-build-alpine"
        env_digest = os.environ.get("CI_IMAGE_ALPINE_DIGEST", "").strip()
        env_ref = os.environ.get("CI_IMAGE_ALPINE", "").strip()
        paths = (
            "docker/ci/Dockerfile.alpine",
            "scripts/ci/install-alpine-native-toolchain.sh",
            "scripts/ci/build-sherpa-onnx-musl-libs.sh",
        )
    else:
        name = "ci-build"
        env_digest = os.environ.get("CI_IMAGE_DIGEST", "").strip()
        env_ref = os.environ.get("CI_IMAGE", "").strip()
        paths = ("docker/ci/Dockerfile",)

    # Explicit sha256 digest from docker inspect / build-push output.
    if env_digest.startswith("sha256:"):
        return f"ghcr.io/{repo}/{name}@{env_digest}"
    if "@sha256:" in env_ref:
        return env_ref
    # Immutable tag (40-char git SHA) — acceptable identity.
    if env_ref and re.search(r":[0-9a-f]{40}$", env_ref):
        return env_ref

    rows = [[sha256_file(root / p), p] for p in paths]
    content = sha256_text(stable_json(rows))[:16]
    return f"ghcr.io/{repo}/{name}@content:{content}"


def declared_tool_identity(target: str, root: Path | None = None) -> dict[str, str]:
    """Planner/builder-shared tool contract (no host rustc -Vv).

    Mutable runner labels (macos-latest / windows-latest) are recorded
    explicitly so digests move when the job layout changes. CI images use a
    content digest (or registry digest / SHA tag) — never bare `:latest` as
    identity. Resolved rustc/cargo/node versions are stored separately on the
    provenance manifest.
    """
    base = root or repo_root()
    repo = os.environ.get("GITHUB_REPOSITORY", "local/node-webrtc-rust")
    ci_image = _ci_image_content_ref(base, repo, "glibc")
    alpine = _ci_image_content_ref(base, repo, "alpine")
    zig = os.environ.get("NATIVE_ZIG_IDENTITY", "0.14.1")
    napi = _napi_cli_identity_default(root)

    if target == "x86_64-unknown-linux-musl":
        return {
            "rustc": "stable",
            "cargo": "stable",
            "node": "24",
            "image": alpine,
            "runner": "self-hosted+ci-build-alpine",
            "host_sdk": "alpine",
            "zig": "none",
            "napi_cli": napi,
        }
    if target == "x86_64-unknown-linux-gnu":
        return {
            "rustc": "stable",
            "cargo": "stable",
            "node": "20",
            "image": ci_image,
            "runner": "self-hosted+ci-build",
            "host_sdk": "ubuntu-24.04",
            "zig": "none",
            "napi_cli": napi,
        }
    if target == "aarch64-unknown-linux-gnu":
        return {
            "rustc": "stable",
            "cargo": "stable",
            "node": "20",
            "image": "host",
            "runner": "ubuntu-24.04-arm",
            "host_sdk": "ubuntu-24.04-arm",
            "zig": "none",
            "napi_cli": napi,
        }
    if target.endswith("apple-darwin"):
        return {
            "rustc": "stable",
            "cargo": "stable",
            "node": "20",
            "image": "host",
            "runner": "macos-latest",
            "host_sdk": "macos",
            "zig": zig,
            "napi_cli": napi,
        }
    if target == "x86_64-pc-windows-msvc":
        return {
            "rustc": "stable",
            "cargo": "stable",
            "node": "20",
            "image": "host",
            "runner": "windows-latest",
            "host_sdk": "msvc",
            "zig": "none",
            "napi_cli": napi,
        }
    raise SystemExit(f"declared_tool_identity: unknown target {target!r}")


def _use_declared_tools() -> bool:
    mode = os.environ.get("NATIVE_TOOL_MODE", "").strip().lower()
    if mode in ("declared", "1", "true", "yes"):
        return True
    if mode in ("unresolved", "0", "false", "no"):
        return False
    return os.environ.get("GITHUB_ACTIONS", "").lower() == "true"


def tool_identity_for_fingerprint(target: str, root: Path | None = None) -> dict[str, str]:
    """Fingerprint tool slots.

    In declared/CI mode, always use the per-target declared contract so
    aggregate digests and plan/build keys cannot be polluted by ambient
    NATIVE_* exports from a previous target. Locally, explicit env wins
    over unresolved.
    """
    keys = (
        ("rustc", "NATIVE_RUSTC_IDENTITY"),
        ("cargo", "NATIVE_CARGO_IDENTITY"),
        ("node", "NATIVE_NODE_IDENTITY"),
        ("image", "NATIVE_IMAGE_DIGEST"),
        ("runner", "NATIVE_RUNNER_LABEL"),
        ("host_sdk", "NATIVE_HOST_SDK_IDENTITY"),
        ("zig", "NATIVE_ZIG_IDENTITY"),
        ("napi_cli", "NATIVE_NAPI_CLI_IDENTITY"),
    )
    if _use_declared_tools():
        return declared_tool_identity(target, root)
    explicit = {k: os.environ[env] for k, env in keys if env in os.environ and os.environ[env] != ""}
    out = {k: "unresolved" for k, _ in keys}
    out.update(explicit)
    return out


def resolved_tool_identity_from_env() -> dict[str, str]:
    """Optional host-resolved identities for provenance (not fingerprint keys)."""
    mapping = {
        "rustc": "NATIVE_RESOLVED_RUSTC",
        "cargo": "NATIVE_RESOLVED_CARGO",
        "node": "NATIVE_RESOLVED_NODE",
        "napi_cli": "NATIVE_RESOLVED_NAPI_CLI",
        "image": "NATIVE_RESOLVED_IMAGE",
        "runner": "NATIVE_RESOLVED_RUNNER",
        "host_sdk": "NATIVE_RESOLVED_HOST_SDK",
        "zig": "NATIVE_RESOLVED_ZIG",
    }
    out = {k: os.environ[env] for k, env in mapping.items() if env in os.environ and os.environ[env]}
    return out


def build_env_contract(target: str, profile: str, root: Path | None = None) -> dict[str, Any]:
    """Normalized build-time env that affects .node bytes.

    Absent vs empty is preserved for SHERPA_ONNX_LIB_DIR.
    Tool identities: CI uses declared per-target contracts so plan/build agree;
    local tests default to 'unresolved' unless NATIVE_TOOL_MODE=declared.
    """
    features = profile_features(profile)
    sherpa = os.environ.get("NATIVE_SHERPA_ONNX_LIB_DIR")
    if sherpa is None:
        # Musl release builds set a concrete lib dir; other targets leave it unset.
        if "musl" in target:
            if _use_declared_tools():
                sherpa_state: dict[str, Any] = {
                    "state": "set",
                    "value": "/opt/sherpa-musl/lib",
                }
            else:
                sherpa_state = {"state": "required-in-ci", "value": None}
        else:
            sherpa_state = {"state": "unset", "value": None}
    elif sherpa == "":
        sherpa_state = {"state": "empty", "value": ""}
    else:
        sherpa_state = {"state": "set", "value": sherpa}

    return {
        "CMAKE_POLICY_VERSION_MINIMUM": "3.5",
        "OPUS_STATIC": "1",
        "SHERPA_ONNX_LIB_DIR": sherpa_state,
        "napi_features_flag": features,
        "tool_identity": tool_identity_for_fingerprint(target, root),
    }


def run_cargo_metadata(root: Path, target: str, features: list[str]) -> dict[str, Any]:
    cmd = [
        "cargo",
        "metadata",
        "--format-version=1",
        "--manifest-path",
        str(root / "packages/bindings/Cargo.toml"),
        "--filter-platform",
        target,
    ]
    if features:
        cmd.extend(["--features", ",".join(features)])
    try:
        raw = subprocess.check_output(cmd, cwd=str(root), text=True)
    except FileNotFoundError as exc:
        raise SystemExit("native_build_contract: cargo not found on PATH") from exc
    except subprocess.CalledProcessError as exc:
        raise SystemExit(
            f"native_build_contract: cargo metadata failed for target={target} "
            f"features={features}: {exc}"
        ) from exc
    return json.loads(raw)


def local_dependency_closure(
    metadata: dict[str, Any], root_package: str = BINDINGS_PACKAGE
) -> list[dict[str, Any]]:
    """Walk resolve graph from bindings; keep path/source-null packages only."""
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    root_id = None
    for pkg in metadata["packages"]:
        if pkg["name"] == root_package:
            root_id = pkg["id"]
            break
    if root_id is None:
        raise SystemExit(f"native_build_contract: package {root_package!r} not in cargo metadata")
    if root_id not in nodes:
        raise SystemExit(f"native_build_contract: resolve node missing for {root_package}")

    seen: set[str] = set()
    order: list[dict[str, Any]] = []
    q: deque[str] = deque([root_id])
    while q:
        pkg_id = q.popleft()
        if pkg_id in seen:
            continue
        seen.add(pkg_id)
        pkg = packages[pkg_id]
        node = nodes[pkg_id]
        if pkg.get("source") is None:
            manifest = Path(pkg["manifest_path"])
            crate_root = manifest.parent
            order.append(
                {
                    "name": pkg["name"],
                    "version": pkg["version"],
                    "manifest_path": str(manifest),
                    "crate_root": str(crate_root),
                    "features": sorted(node.get("features") or []),
                }
            )
        for dep in node.get("deps") or []:
            q.append(dep["pkg"])
    order.sort(key=lambda c: c["name"])
    return order


def hash_tree_files(root: Path, crate_root: Path) -> list[list[str]]:
    """Hash Cargo.toml, build.rs, and all .rs under src/ (sorted)."""
    rows: list[list[str]] = []
    manifest = crate_root / "Cargo.toml"
    if not manifest.is_file():
        raise SystemExit(f"native_build_contract: missing {manifest}")
    rows.append([sha256_file(manifest), str(manifest.relative_to(root))])

    build_rs = crate_root / "build.rs"
    if build_rs.is_file():
        rows.append([sha256_file(build_rs), str(build_rs.relative_to(root))])

    src = crate_root / "src"
    if src.is_dir():
        for path in sorted(src.rglob("*.rs")):
            if path.is_file():
                rows.append([sha256_file(path), str(path.relative_to(root))])
    return rows


def hash_paths(root: Path, rel_paths: Iterable[str]) -> list[list[str]]:
    rows: list[list[str]] = []
    for rel in rel_paths:
        path = root / rel
        if not path.is_file():
            raise SystemExit(f"native_build_contract: missing required path {rel}")
        rows.append([sha256_file(path), rel])
    return rows


def target_native_paths(target: str) -> list[str]:
    paths: list[str] = []
    if "linux" in target and "musl" in target:
        paths.extend(MUSL_ONLY_PATHS)
    elif "linux" in target:
        paths.extend(GNU_LINUX_PATHS)
    # darwin / windows: host action covered in RECIPE_PATHS; no musl/gnu docker inputs
    return paths


def napi_cli_lock_digest(root: Path) -> str:
    """Hash the lockfile slice for @napi-rs/cli (build tool, not npm package version)."""
    lock = root / "package-lock.json"
    if not lock.is_file():
        raise SystemExit("native_build_contract: missing package-lock.json")
    data = json.loads(lock.read_text(encoding="utf-8"))
    entry = (data.get("packages") or {}).get("node_modules/@napi-rs/cli")
    if not entry:
        raise SystemExit("native_build_contract: @napi-rs/cli missing from package-lock.json")
    return sha256_text(stable_json(entry))


def workspace_cargo_version(root: Path) -> str:
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    # Prefer [workspace.package] version = "…"
    m = re.search(
        r"\[workspace\.package\][^\[]*?^version\s*=\s*\"([^\"]+)\"",
        text,
        flags=re.M | re.S,
    )
    if m:
        return m.group(1)
    raise SystemExit("native_build_contract: could not parse workspace.package version")


def package_json_without_version(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    data.pop("version", None)
    return data


def build_native_contract(root: Path, target: str, profile: str) -> dict[str, Any]:
    if target not in RELEASE_TARGET_MAP:
        raise SystemExit(
            f"native_build_contract: target {target!r} is not a release target. "
            f"Canonical six: {list(RELEASE_TARGET_MAP)}"
        )
    features = profile_features(profile)
    metadata = run_cargo_metadata(root, target, features)
    local = local_dependency_closure(metadata)
    if not any(c["name"] == BINDINGS_PACKAGE for c in local):
        raise SystemExit("native_build_contract: bindings package missing from local closure")
    if any(c["name"] == "node-webrtc-rust-signaling" for c in local):
        raise SystemExit(
            "native_build_contract: signaling unexpectedly in bindings local closure"
        )

    crate_rows: list[dict[str, Any]] = []
    for crate in local:
        crate_root = Path(crate["crate_root"])
        crate_rows.append(
            {
                "name": crate["name"],
                "version": crate["version"],
                "features": crate["features"],
                "files": hash_tree_files(root, crate_root),
            }
        )

    contract: dict[str, Any] = {
        "schema": "node-webrtc-rust.native-input-contract/v1",
        "target": target,
        "profile": profile,
        "features": features,
        "cache_epoch": read_cache_epoch(root),
        "workspace_cargo_version": workspace_cargo_version(root),
        "cargo_lock": sha256_file(root / "Cargo.lock"),
        "workspace_manifest": sha256_file(root / "Cargo.toml"),
        "local_crates": crate_rows,
        "recipe_files": hash_paths(root, RECIPE_PATHS),
        "target_files": hash_paths(root, target_native_paths(target)),
        "napi_cli_lock": napi_cli_lock_digest(root),
        "build_env": build_env_contract(target, profile, root),
        "npm_package": RELEASE_TARGET_MAP[target]["npm_package"],
    }
    return contract


def native_input_digest(root: Path, target: str, profile: str) -> str:
    return sha256_text(stable_json(build_native_contract(root, target, profile)))


def cache_key_for_target(root: Path, target: str, profile: str) -> str:
    digest = native_input_digest(root, target, profile)
    return f"{NATIVE_CACHE_KEY_PREFIX}-{profile}-{target}-{digest}"


def aggregate_native_digest(root: Path, profile: str) -> str:
    targets = list_release_targets(root)
    return aggregate_digest_from_digests(targets, per_target_digests(root, profile))


def per_target_digests(root: Path, profile: str) -> dict[str, str]:
    return {
        target: native_input_digest(root, target, profile)
        for target in list_release_targets(root)
    }


def aggregate_digest_from_digests(
    targets: list[str], digests: dict[str, str]
) -> str:
    lines = "\n".join(f"{target}={digests[target]}" for target in targets)
    return sha256_text(lines + "\n")


def build_distribution_contract(root: Path, target: str) -> dict[str, Any]:
    mapping = RELEASE_TARGET_MAP[target]
    npm_pkg_path = root / "packages/bindings/npm" / mapping["npm_dir"] / "package.json"
    bindings_pkg = root / "packages/bindings/package.json"
    if not npm_pkg_path.is_file():
        raise SystemExit(f"native_build_contract: missing platform package {npm_pkg_path}")

    surface = hash_paths(root, DISTRIBUTION_SURFACE_PATHS)
    return {
        "schema": "node-webrtc-rust.native-distribution-contract/v1",
        "target": target,
        "npm_package": mapping["npm_package"],
        "node_basename": mapping["node_basename"],
        "bindings_package_json": package_json_without_version(bindings_pkg),
        # Distribution includes npm version (unlike native-byte digest).
        "platform_package_json": json.loads(npm_pkg_path.read_text(encoding="utf-8")),
        "surface_files": surface,
        "bindings_package_json_full_version": json.loads(
            bindings_pkg.read_text(encoding="utf-8")
        ).get("version"),
    }


def distribution_digest(root: Path, target: str) -> str:
    return sha256_text(stable_json(build_distribution_contract(root, target)))


def napi_surface_digest(root: Path) -> str:
    rows = hash_paths(root, DISTRIBUTION_SURFACE_PATHS)
    return sha256_text(stable_json(rows))


def find_node_artifact(root: Path, target: str) -> Path | None:
    mapping = RELEASE_TARGET_MAP[target]
    candidates = [
        root / "packages/bindings" / mapping["node_basename"],
        root / "packages/bindings/npm" / mapping["npm_dir"] / mapping["node_basename"],
        root / "packages/bindings/artifacts" / f"bindings-{target}" / mapping["node_basename"],
        root / "packages/bindings/prebuilt" / f"bindings-{target}" / mapping["node_basename"],
    ]
    # Host-style local build may emit generic name.
    if target.endswith("linux-gnu") and "x86_64" in target:
        candidates.append(root / "packages/bindings/node-webrtc-rust.node")
    for path in candidates:
        if path.is_file():
            return path
    return None


def collect_dynamic_dependencies(node_path: Path) -> dict[str, Any]:
    """Best-effort dynamic dependency closure; host-dependent."""
    system = sys.platform
    try:
        if system.startswith("linux"):
            # Prefer readelf NEEDED; fall back to ldd.
            try:
                out = subprocess.check_output(
                    ["readelf", "-d", str(node_path)], text=True, stderr=subprocess.DEVNULL
                )
                needed = sorted(
                    re.findall(r"Shared library:\s*\[([^\]]+)\]", out)
                )
                return {"collector": "readelf", "needed": needed}
            except (FileNotFoundError, subprocess.CalledProcessError):
                out = subprocess.check_output(
                    ["ldd", str(node_path)], text=True, stderr=subprocess.STDOUT
                )
                libs = []
                for line in out.splitlines():
                    line = line.strip()
                    if " => " in line:
                        libs.append(line.split(" => ", 1)[0].strip())
                    elif line.endswith(" (0x") or line.startswith("/"):
                        libs.append(line.split()[0])
                return {"collector": "ldd", "needed": sorted(set(libs))}
        if system == "darwin":
            out = subprocess.check_output(
                ["otool", "-L", str(node_path)], text=True, stderr=subprocess.DEVNULL
            )
            libs = []
            for i, line in enumerate(out.splitlines()):
                if i == 0:
                    continue
                libs.append(line.strip().split(" (", 1)[0])
            return {"collector": "otool", "needed": libs}
    except (FileNotFoundError, subprocess.CalledProcessError) as exc:
        return {"collector": "unavailable", "needed": [], "error": str(exc)}
    return {"collector": "unsupported-platform", "needed": []}


def produce_manifest(
    root: Path,
    target: str,
    profile: str,
    output: Path,
    node_path: Path | None = None,
) -> dict[str, Any]:
    mapping = RELEASE_TARGET_MAP[target]
    artifact = node_path or find_node_artifact(root, target)
    if artifact is None or not artifact.is_file():
        raise SystemExit(
            f"native_build_contract: no .node artifact found for {target}. "
            f"Expected {mapping['node_basename']} under packages/bindings/"
        )

    input_digest = native_input_digest(root, target, profile)
    dist_digest = distribution_digest(root, target)
    node_sha = sha256_file(artifact)
    node_size = artifact.stat().st_size
    dyn = collect_dynamic_dependencies(artifact)

    producer = {
        "git_sha": os.environ.get("NATIVE_PRODUCER_GIT_SHA")
        or os.environ.get("GITHUB_SHA"),
        "run_id": os.environ.get("NATIVE_PRODUCER_RUN_ID")
        or os.environ.get("GITHUB_RUN_ID"),
        "workflow": os.environ.get("NATIVE_PRODUCER_WORKFLOW")
        or os.environ.get("GITHUB_WORKFLOW"),
    }
    # Drop nulls for stable empty-local manifests
    producer = {k: v for k, v in producer.items() if v}

    tool = build_env_contract(target, profile, root)["tool_identity"]
    resolved = resolved_tool_identity_from_env()
    manifest: dict[str, Any] = {
        "schema": SCHEMA,
        "target": target,
        "profile": profile,
        "features": profile_features(profile),
        "input_digest": input_digest,
        "distribution_digest": dist_digest,
        "cache_epoch": read_cache_epoch(root),
        "npm_package": mapping["npm_package"],
        "node_artifact": {
            "path": _relpath_or_abs(artifact, root),
            "sha256": node_sha,
            "size": node_size,
            "basename": artifact.name,
        },
        "napi_surface_digest": napi_surface_digest(root),
        "producer": producer,
        "tool_identity": tool,
        "dynamic_dependencies": dyn,
    }
    if resolved:
        manifest["tool_identity_resolved"] = resolved

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(stable_json(manifest) + "\n", encoding="utf-8")
    return manifest


def validate_manifest(
    root: Path,
    manifest_path: Path,
    *,
    recompute_input: bool = True,
    require_node: bool = True,
    node_path: Path | None = None,
) -> None:
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise SystemExit(f"validate-manifest: malformed JSON: {exc}") from exc

    if not isinstance(manifest, dict):
        raise SystemExit("validate-manifest: manifest root must be an object")

    required = [
        "schema",
        "target",
        "profile",
        "features",
        "input_digest",
        "node_artifact",
        "napi_surface_digest",
    ]
    missing = [k for k in required if k not in manifest]
    if missing:
        raise SystemExit(f"validate-manifest: missing required fields: {missing}")

    if manifest["schema"] != SCHEMA:
        raise SystemExit(
            f"validate-manifest: unsupported schema {manifest['schema']!r} "
            f"(expected {SCHEMA!r})"
        )

    target = manifest["target"]
    profile = manifest["profile"]
    if target not in RELEASE_TARGET_MAP:
        raise SystemExit(f"validate-manifest: non-release target {target!r}")
    if profile not in ("debug", "release"):
        raise SystemExit(f"validate-manifest: invalid profile {profile!r}")

    expected_features = profile_features(profile)
    if list(manifest["features"]) != expected_features:
        raise SystemExit(
            f"validate-manifest: features mismatch: manifest={manifest['features']} "
            f"expected={expected_features}"
        )

    node_meta = manifest["node_artifact"]
    if not isinstance(node_meta, dict):
        raise SystemExit("validate-manifest: node_artifact must be an object")
    for key in ("sha256", "size"):
        if key not in node_meta:
            raise SystemExit(f"validate-manifest: node_artifact missing {key}")

    if not re.fullmatch(r"[0-9a-f]{64}", str(node_meta["sha256"])):
        raise SystemExit("validate-manifest: node_artifact.sha256 must be 64 lowercase hex")
    if not re.fullmatch(r"[0-9a-f]{64}", str(manifest["input_digest"])):
        raise SystemExit("validate-manifest: input_digest must be 64 lowercase hex")

    if require_node:
        if node_path is not None:
            artifact = node_path
            if not artifact.is_file():
                raise SystemExit(
                    f"validate-manifest: explicit .node missing for {target}: {artifact}"
                )
        else:
            rel = node_meta.get("path")
            artifact = (
                Path(rel) if rel and Path(rel).is_absolute() else root / str(rel or "")
            )
            if not rel or not artifact.is_file():
                # Try canonical locations for standalone/cache validation.
                found = find_node_artifact(root, target)
                if found is None:
                    raise SystemExit(
                        f"validate-manifest: .node missing for {target} (path={rel!r})"
                    )
                artifact = found
        actual_sha = sha256_file(artifact)
        actual_size = artifact.stat().st_size
        if actual_sha != node_meta["sha256"]:
            raise SystemExit(
                f"validate-manifest: .node sha256 mismatch\n"
                f"  path: {artifact}\n"
                f"  manifest: {node_meta['sha256']}\n"
                f"  actual:   {actual_sha}"
            )
        if int(node_meta["size"]) != actual_size:
            raise SystemExit(
                f"validate-manifest: .node size mismatch "
                f"(manifest={node_meta['size']} actual={actual_size})"
            )

        # If dynamic deps were recorded with a real collector, re-collect and compare.
        recorded = manifest.get("dynamic_dependencies") or {}
        collector = recorded.get("collector")
        if collector in ("readelf", "ldd", "otool"):
            current = collect_dynamic_dependencies(artifact)
            if current.get("collector") == collector:
                if current.get("needed") != recorded.get("needed"):
                    raise SystemExit(
                        "validate-manifest: dynamic dependency closure mismatch\n"
                        f"  manifest: {recorded.get('needed')}\n"
                        f"  actual:   {current.get('needed')}"
                    )

    if recompute_input:
        expected = native_input_digest(root, target, profile)
        if manifest["input_digest"] != expected:
            raise SystemExit(
                "validate-manifest: input_digest does not match recomputed contract\n"
                f"  manifest:  {manifest['input_digest']}\n"
                f"  recomputed:{expected}"
            )

        expected_surface = napi_surface_digest(root)
        if manifest.get("napi_surface_digest") != expected_surface:
            raise SystemExit(
                "validate-manifest: napi_surface_digest mismatch "
                "(index.js / index.d.ts changed since produce)"
            )

        if "distribution_digest" in manifest:
            expected_dist = distribution_digest(root, target)
            if manifest["distribution_digest"] != expected_dist:
                raise SystemExit(
                    "validate-manifest: distribution_digest mismatch\n"
                    f"  manifest:  {manifest['distribution_digest']}\n"
                    f"  recomputed:{expected_dist}"
                )


def assemble_native_bundle(
    root: Path,
    profile: str,
    artifacts_root: Path,
    output_dir: Path,
) -> dict[str, Any]:
    """Assemble six-target bundle from downloaded bindings-* dirs.

    Expected layout under artifacts_root:
      bindings-<triple>/*.node
      bindings-<triple>/manifest.json (optional; produced if missing when .node present)
    """
    targets = list_release_targets(root)
    digests = per_target_digests(root, profile)
    aggregate = aggregate_digest_from_digests(targets, digests)
    output_dir.mkdir(parents=True, exist_ok=True)

    target_meta: dict[str, Any] = {}
    for target in targets:
        mapping = RELEASE_TARGET_MAP[target]
        src_dir = artifacts_root / f"bindings-{target}"
        if not src_dir.is_dir():
            # Also accept flat artifact download layouts
            alt = artifacts_root / target
            src_dir = alt if alt.is_dir() else src_dir
        if not src_dir.is_dir():
            raise SystemExit(f"assemble-bundle: missing directory for {target}: {src_dir}")

        preferred = src_dir / mapping["node_basename"]
        if not preferred.is_file():
            raise SystemExit(
                f"assemble-bundle: canonical .node missing for {target}: {preferred}"
            )
        node_path = preferred

        dest = output_dir / target
        dest.mkdir(parents=True, exist_ok=True)
        dest_node = dest / mapping["node_basename"]
        dest_node.write_bytes(node_path.read_bytes())

        manifest_src = src_dir / "manifest.json"
        manifest_dest = dest / "manifest.json"
        if manifest_src.is_file():
            manifest_dest.write_bytes(manifest_src.read_bytes())
        else:
            produce_manifest(root, target, profile, manifest_dest, node_path=dest_node)

        # Producer manifests record paths in the producer workspace. Validate the
        # bytes copied into the portable bundle, never that stale source path.
        validate_manifest(
            root,
            manifest_dest,
            recompute_input=True,
            require_node=True,
            node_path=dest_node,
        )
        manifest = json.loads(manifest_dest.read_text(encoding="utf-8"))
        if manifest["target"] != target or manifest["profile"] != profile:
            raise SystemExit(
                "assemble-bundle: manifest identity mismatch "
                f"target={manifest['target']!r} profile={manifest['profile']!r} "
                f"expected_target={target!r} expected_profile={profile!r}"
            )
        if manifest["input_digest"] != digests[target]:
            raise SystemExit(
                f"assemble-bundle: input_digest mismatch for {target}\n"
                f"  manifest: {manifest['input_digest']}\n"
                f"  current:  {digests[target]}"
            )
        target_meta[target] = {
            "input_digest": digests[target],
            "node_sha256": manifest["node_artifact"]["sha256"],
            "node_size": manifest["node_artifact"]["size"],
            "npm_package": mapping["npm_package"],
            "node_basename": mapping["node_basename"],
        }

    producer = {
        "git_sha": os.environ.get("NATIVE_PRODUCER_GIT_SHA")
        or os.environ.get("GITHUB_SHA"),
        "run_id": os.environ.get("NATIVE_PRODUCER_RUN_ID")
        or os.environ.get("GITHUB_RUN_ID"),
        "workflow": os.environ.get("NATIVE_PRODUCER_WORKFLOW")
        or os.environ.get("GITHUB_WORKFLOW")
        or MAIN_WORKFLOW_NAME,
    }
    producer = {k: v for k, v in producer.items() if v}

    meta = {
        "schema": BUNDLE_SCHEMA,
        "profile": profile,
        "aggregate_digest": aggregate,
        "targets": target_meta,
        "producer": producer,
    }
    (output_dir / "meta.json").write_text(stable_json(meta) + "\n", encoding="utf-8")
    return meta


def validate_native_bundle(
    root: Path,
    bundle_dir: Path,
    profile: str,
    *,
    expect_aggregate: str | None = None,
) -> dict[str, Any]:
    meta_path = bundle_dir / "meta.json"
    if not meta_path.is_file():
        raise SystemExit(f"validate-bundle: missing {meta_path}")
    try:
        meta = json.loads(meta_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise SystemExit(f"validate-bundle: malformed meta.json: {exc}") from exc

    if meta.get("schema") != BUNDLE_SCHEMA:
        raise SystemExit(
            f"validate-bundle: unsupported schema {meta.get('schema')!r} "
            f"(expected {BUNDLE_SCHEMA!r})"
        )
    if meta.get("profile") != profile:
        raise SystemExit(
            f"validate-bundle: profile mismatch manifest={meta.get('profile')} expected={profile}"
        )

    expected_targets = list_release_targets(root)
    targets = meta.get("targets")
    if not isinstance(targets, dict):
        raise SystemExit("validate-bundle: meta.targets must be an object")
    missing = [t for t in expected_targets if t not in targets]
    extra = [t for t in targets if t not in expected_targets]
    if missing or extra:
        raise SystemExit(
            f"validate-bundle: target set incomplete missing={missing} extra={extra}"
        )

    current_digests = per_target_digests(root, profile)
    current_aggregate = aggregate_digest_from_digests(expected_targets, current_digests)
    if expect_aggregate is None:
        expect_aggregate = current_aggregate
    if meta.get("aggregate_digest") != expect_aggregate:
        raise SystemExit(
            "validate-bundle: aggregate_digest mismatch\n"
            f"  meta:     {meta.get('aggregate_digest')}\n"
            f"  expected: {expect_aggregate}"
        )
    if current_aggregate != expect_aggregate:
        raise SystemExit(
            "validate-bundle: current workspace aggregate differs from expected\n"
            f"  current:  {current_aggregate}\n"
            f"  expected: {expect_aggregate}"
        )

    for target in expected_targets:
        tdir = bundle_dir / target
        mapping = RELEASE_TARGET_MAP[target]
        node_path = tdir / mapping["node_basename"]
        manifest_path = tdir / "manifest.json"
        if not manifest_path.is_file():
            raise SystemExit(f"validate-bundle: missing manifest for {target}")
        validate_manifest(
            root,
            manifest_path,
            recompute_input=True,
            require_node=True,
            node_path=node_path,
        )
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        target_meta = targets[target]
        if not isinstance(target_meta, dict):
            raise SystemExit(f"validate-bundle: meta target entry is not an object: {target}")
        if manifest["target"] != target or manifest["profile"] != profile:
            raise SystemExit(f"validate-bundle: manifest identity mismatch for {target}")
        if target_meta.get("node_basename") != mapping["node_basename"]:
            raise SystemExit(f"validate-bundle: node_basename mismatch for {target}")
        if target_meta.get("npm_package") != mapping["npm_package"]:
            raise SystemExit(f"validate-bundle: npm_package mismatch for {target}")
        if target_meta.get("node_sha256") != manifest["node_artifact"]["sha256"]:
            raise SystemExit(f"validate-bundle: meta/manifest node sha256 drift for {target}")
        if target_meta.get("node_size") != manifest["node_artifact"]["size"]:
            raise SystemExit(f"validate-bundle: meta/manifest node size drift for {target}")
        if manifest["input_digest"] != target_meta.get("input_digest"):
            raise SystemExit(f"validate-bundle: meta/manifest digest drift for {target}")
        if manifest["input_digest"] != current_digests[target]:
            raise SystemExit(
                f"validate-bundle: target {target} does not match current fingerprint"
            )
    return meta


def stage_bundle_to_bindings_artifacts(bundle_dir: Path, out_root: Path) -> None:
    """Copy bundle targets into bindings-<triple>/ dirs for napi artifacts / upload."""
    meta = json.loads((bundle_dir / "meta.json").read_text(encoding="utf-8"))
    if meta.get("schema") != BUNDLE_SCHEMA:
        raise SystemExit("stage-bundle: unsupported bundle schema")
    profile = meta.get("profile")
    if profile not in ("debug", "release"):
        raise SystemExit("stage-bundle: invalid profile")
    targets = meta.get("targets")
    expected_targets = list(RELEASE_TARGET_MAP.keys())
    if not isinstance(targets, dict) or set(targets) != set(expected_targets):
        raise SystemExit("stage-bundle: invalid target set")
    for target in expected_targets:
        info = targets[target]
        mapping = RELEASE_TARGET_MAP[target]
        if not isinstance(info, dict):
            raise SystemExit(f"stage-bundle: invalid target metadata for {target}")
        if info.get("node_basename") != mapping["node_basename"]:
            raise SystemExit(f"stage-bundle: node_basename mismatch for {target}")
        if info.get("npm_package") != mapping["npm_package"]:
            raise SystemExit(f"stage-bundle: npm_package mismatch for {target}")
        src = bundle_dir / target
        node_name = mapping["node_basename"]
        src_node = src / node_name
        manifest_path = src / "manifest.json"
        if not src_node.is_file() or not manifest_path.is_file():
            raise SystemExit(f"stage-bundle: incomplete target directory for {target}")
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if manifest.get("target") != target or manifest.get("profile") != profile:
            raise SystemExit(f"stage-bundle: manifest identity mismatch for {target}")
        node_meta = manifest.get("node_artifact")
        if not isinstance(node_meta, dict):
            raise SystemExit(f"stage-bundle: invalid node metadata for {target}")
        actual_sha = sha256_file(src_node)
        actual_size = src_node.stat().st_size
        if node_meta.get("sha256") != actual_sha or info.get("node_sha256") != actual_sha:
            raise SystemExit(f"stage-bundle: node sha256 mismatch for {target}")
        if node_meta.get("size") != actual_size or info.get("node_size") != actual_size:
            raise SystemExit(f"stage-bundle: node size mismatch for {target}")
        dest = out_root / f"bindings-{target}"
        dest.mkdir(parents=True, exist_ok=True)
        (dest / node_name).write_bytes(src_node.read_bytes())
        (dest / "manifest.json").write_bytes(manifest_path.read_bytes())


def check_release_targets(root: Path) -> None:
    """Release-contract completeness. Does not police loader fallback requires."""
    targets = list_release_targets(root)
    bindings_pkg = json.loads(
        (root / "packages/bindings/package.json").read_text(encoding="utf-8")
    )
    optional = bindings_pkg.get("optionalDependencies") or {}
    npm_root = root / "packages/bindings/npm"
    if not npm_root.is_dir():
        raise SystemExit("check-release-targets: packages/bindings/npm missing")

    npm_dirs = sorted(
        p.name for p in npm_root.iterdir() if p.is_dir() and (p / "package.json").is_file()
    )
    expected_dirs = sorted(RELEASE_TARGET_MAP[t]["npm_dir"] for t in targets)
    if npm_dirs != expected_dirs:
        raise SystemExit(
            "check-release-targets: npm platform directories must match the six "
            f"release targets exactly.\n  found:    {npm_dirs}\n  expected: {expected_dirs}"
        )

    expected_pkgs = {RELEASE_TARGET_MAP[t]["npm_package"] for t in targets}
    optional_pkgs = set(optional)
    if optional_pkgs != expected_pkgs:
        raise SystemExit(
            "check-release-targets: bindings optionalDependencies must be exactly "
            f"the six release platform packages.\n"
            f"  found:    {sorted(optional_pkgs)}\n"
            f"  expected: {sorted(expected_pkgs)}"
        )

    for target in targets:
        mapping = RELEASE_TARGET_MAP[target]
        pkg_path = npm_root / mapping["npm_dir"] / "package.json"
        data = json.loads(pkg_path.read_text(encoding="utf-8"))
        if data.get("name") != mapping["npm_package"]:
            raise SystemExit(
                f"check-release-targets: {pkg_path} name={data.get('name')!r} "
                f"expected {mapping['npm_package']!r}"
            )
        main = data.get("main")
        if main != mapping["node_basename"]:
            raise SystemExit(
                f"check-release-targets: {pkg_path} main={main!r} "
                f"expected {mapping['node_basename']!r}"
            )
        if mapping["npm_package"] not in optional:
            raise SystemExit(
                f"check-release-targets: missing optionalDependency {mapping['npm_package']}"
            )

    # Ensure plan-native-builds consumes the canonical list (static check).
    plan = (root / "scripts/ci/plan-native-builds.sh").read_text(encoding="utf-8")
    if "list-release-targets.sh" not in plan:
        raise SystemExit(
            "check-release-targets: plan-native-builds.sh must call list-release-targets.sh"
        )

    print("ok: six release targets, npm dirs, and optionalDependencies are complete")
    print("note: packages/bindings/index.js may require additional fallback packages; "
          "those are outside the release contract and are not planned for publish.")


def cmd_fingerprint(args: argparse.Namespace) -> int:
    root = repo_root()
    digest = native_input_digest(root, args.target, args.profile)
    sys.stdout.write(digest + "\n")
    return 0


def cmd_cache_key(args: argparse.Namespace) -> int:
    root = repo_root()
    sys.stdout.write(cache_key_for_target(root, args.target, args.profile) + "\n")
    return 0


def cmd_aggregate_digest(args: argparse.Namespace) -> int:
    root = repo_root()
    sys.stdout.write(aggregate_native_digest(root, args.profile) + "\n")
    return 0


def cmd_distribution_digest(args: argparse.Namespace) -> int:
    root = repo_root()
    sys.stdout.write(distribution_digest(root, args.target) + "\n")
    return 0


def cmd_contract_json(args: argparse.Namespace) -> int:
    root = repo_root()
    contract = build_native_contract(root, args.target, args.profile)
    if args.with_digests:
        contract["input_digest"] = sha256_text(stable_json(contract))
        contract["distribution_digest"] = distribution_digest(root, args.target)
    sys.stdout.write(stable_json(contract) + "\n")
    return 0


def cmd_list_local_crates(args: argparse.Namespace) -> int:
    root = repo_root()
    features = profile_features(args.profile)
    metadata = run_cargo_metadata(root, args.target, features)
    crates = local_dependency_closure(metadata)
    for crate in crates:
        rel = os.path.relpath(crate["crate_root"], root)
        sys.stdout.write(
            f"{crate['name']}@{crate['version']} features={','.join(crate['features']) or '-'} path={rel}\n"
        )
    return 0


def cmd_produce_manifest(args: argparse.Namespace) -> int:
    root = repo_root()
    node_path = Path(args.node).resolve() if args.node else None
    produce_manifest(
        root,
        args.target,
        args.profile,
        Path(args.output).resolve(),
        node_path=node_path,
    )
    print(f"wrote {args.output}", file=sys.stderr)
    return 0


def cmd_validate_manifest(args: argparse.Namespace) -> int:
    root = repo_root()
    validate_manifest(
        root,
        Path(args.manifest).resolve(),
        recompute_input=not args.skip_recompute,
        require_node=not args.allow_missing_node,
    )
    print("ok: manifest valid")
    return 0


def cmd_check_release_targets(_: argparse.Namespace) -> int:
    check_release_targets(repo_root())
    return 0


def cmd_assemble_bundle(args: argparse.Namespace) -> int:
    root = repo_root()
    meta = assemble_native_bundle(
        root,
        args.profile,
        Path(args.artifacts_root).resolve(),
        Path(args.output).resolve(),
    )
    print(f"ok: assembled bundle aggregate={meta['aggregate_digest']}", file=sys.stderr)
    sys.stdout.write(meta["aggregate_digest"] + "\n")
    return 0


def cmd_validate_bundle(args: argparse.Namespace) -> int:
    root = repo_root()
    meta = validate_native_bundle(
        root,
        Path(args.bundle).resolve(),
        args.profile,
        expect_aggregate=args.expect_aggregate,
    )
    print(f"ok: bundle valid aggregate={meta['aggregate_digest']}")
    return 0


def cmd_stage_bundle(args: argparse.Namespace) -> int:
    stage_bundle_to_bindings_artifacts(
        Path(args.bundle).resolve(), Path(args.output).resolve()
    )
    print(f"ok: staged bindings-* under {args.output}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)

    def add_target_profile(sp: argparse.ArgumentParser) -> None:
        sp.add_argument("--target", required=True, help="Rust target triple")
        sp.add_argument(
            "--profile",
            default="release",
            choices=("debug", "release"),
            help="Cargo/napi profile (default: release)",
        )

    sp = sub.add_parser("fingerprint", help="Print native input digest")
    add_target_profile(sp)
    sp.set_defaults(func=cmd_fingerprint)

    sp = sub.add_parser("cache-key", help="Print native-v3 Actions cache key")
    add_target_profile(sp)
    sp.set_defaults(func=cmd_cache_key)

    sp = sub.add_parser("aggregate-digest", help="Print six-target aggregate digest")
    sp.add_argument(
        "--profile", default="release", choices=("debug", "release")
    )
    sp.set_defaults(func=cmd_aggregate_digest)

    sp = sub.add_parser("distribution-digest", help="Print distribution digest")
    sp.add_argument("--target", required=True)
    sp.set_defaults(func=cmd_distribution_digest)

    sp = sub.add_parser("contract-json", help="Print native input contract JSON")
    add_target_profile(sp)
    sp.add_argument(
        "--with-digests",
        action="store_true",
        help="Include input_digest and distribution_digest fields",
    )
    sp.set_defaults(func=cmd_contract_json)

    sp = sub.add_parser("list-local-crates", help="List local crates in bindings closure")
    add_target_profile(sp)
    sp.set_defaults(func=cmd_list_local_crates)

    sp = sub.add_parser("produce-manifest", help="Write provenance manifest for a .node")
    add_target_profile(sp)
    sp.add_argument("--output", required=True, help="Output JSON path")
    sp.add_argument("--node", help="Path to .node (optional; auto-discover)")
    sp.set_defaults(func=cmd_produce_manifest)

    sp = sub.add_parser("validate-manifest", help="Validate provenance manifest")
    sp.add_argument("--manifest", required=True)
    sp.add_argument(
        "--skip-recompute",
        action="store_true",
        help="Do not recompute input/distribution digests (checksum-only)",
    )
    sp.add_argument(
        "--allow-missing-node",
        action="store_true",
        help="Skip on-disk .node checksum (schema/field checks only)",
    )
    sp.set_defaults(func=cmd_validate_manifest)

    sp = sub.add_parser(
        "check-release-targets",
        help="Verify six-target release npm mapping completeness",
    )
    sp.set_defaults(func=cmd_check_release_targets)

    sp = sub.add_parser("assemble-bundle", help="Assemble six-target native main bundle")
    sp.add_argument("--profile", default="release", choices=("debug", "release"))
    sp.add_argument("--artifacts-root", required=True)
    sp.add_argument("--output", required=True)
    sp.set_defaults(func=cmd_assemble_bundle)

    sp = sub.add_parser("validate-bundle", help="Validate a native main bundle")
    sp.add_argument("--profile", default="release", choices=("debug", "release"))
    sp.add_argument("--bundle", required=True)
    sp.add_argument("--expect-aggregate", default=None)
    sp.set_defaults(func=cmd_validate_bundle)

    sp = sub.add_parser(
        "stage-bundle",
        help="Copy bundle into bindings-<triple> dirs for upload/publish",
    )
    sp.add_argument("--bundle", required=True)
    sp.add_argument("--output", required=True)
    sp.set_defaults(func=cmd_stage_bundle)

    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    sys.exit(main())
