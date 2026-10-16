#!/usr/bin/env python3
"""Print "true" when BASE..HEAD only changes our own @node-webrtc-rust/* versions.

Used by compute-pr-job-gates.sh to skip native compile + integration on post-release
package-lock PRs. Every changed path must be a package.json or package-lock.json that
exists on both sides, and the files must be equal after removing:
  - the top-level "version" of each package.json / package-lock.json and of each
    workspace entry in package-lock "packages" (keys without "node_modules/");
  - @node-webrtc-rust/* names from dependencies / devDependencies / peerDependencies /
    optionalDependencies maps;
  - package-lock "packages" entries installed from the registry under
    node_modules/@node-webrtc-rust/ (our own published packages).
Anything else (third-party version, new dependency, new workspace, invalid JSON) → "false".
"""
import json
import subprocess
import sys

OWN = "@node-webrtc-rust/"
DEP_KEYS = ("dependencies", "devDependencies", "peerDependencies", "optionalDependencies")
NAMES = ("package.json", "package-lock.json")


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout


def strip_own_deps(obj: dict) -> None:
    for key in DEP_KEYS:
        deps = obj.get(key)
        if isinstance(deps, dict):
            obj[key] = {n: v for n, v in deps.items() if not n.startswith(OWN)}


def normalize(path: str, obj: dict) -> dict:
    obj.pop("version", None)
    strip_own_deps(obj)
    if path.rsplit("/", 1)[-1] == "package-lock.json":
        packages = obj.get("packages")
        if isinstance(packages, dict):
            kept = {}
            for key, entry in packages.items():
                if "node_modules/" + OWN in key:
                    continue
                if isinstance(entry, dict) and "node_modules/" not in key:
                    entry.pop("version", None)
                    strip_own_deps(entry)
                kept[key] = entry
            obj["packages"] = kept
    return obj


def load(rev: str, path: str) -> dict:
    return json.loads(git("show", f"{rev}:{path}"))


def main() -> str:
    if len(sys.argv) != 3:
        return "false"
    base, head = sys.argv[1], sys.argv[2]
    changed = [p for p in git("diff", "--name-only", base, head).splitlines() if p]
    if not changed:
        return "false"
    for path in changed:
        if path.rsplit("/", 1)[-1] not in NAMES:
            return "false"
        before, after = load(base, path), load(head, path)
        if not isinstance(before, dict) or not isinstance(after, dict):
            return "false"
        if normalize(path, before) != normalize(path, after):
            return "false"
    return "true"


if __name__ == "__main__":
    try:
        print(main())
    except Exception:  # noqa: BLE001 — any failure means "run the full pipeline"
        print("false")
