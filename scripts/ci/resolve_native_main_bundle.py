#!/usr/bin/env python3
"""Resolve a reusable native-main-bundle from successful Build & Test (main) runs.

Trust rules (fail closed → no reuse):
  - Only workflow file build-main.yml / name Build & Test (main)
  - Only branch main with conclusion success
  - Artifact name must be native-main-bundle and not expired
  - Prefer exact head_sha == current SHA, else older runs whose meta.aggregate_digest
    matches the current six-target aggregate
  - Downloaded bundle must validate manifests/checksums against the current contract
  - Malformed API JSON, wrong workflow/branch, missing artifact → fallback

Uses GitHub REST via urllib (no gh CLI). Optional --fixture-dir for deterministic tests.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path
from typing import Any, Callable

# Import sibling contract module
sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_build_contract as nbc  # noqa: E402

API_VERSION = "2022-11-28"


class ResolveError(Exception):
    def __init__(self, reason: str, detail: str = ""):
        self.reason = reason
        self.detail = detail
        super().__init__(reason if not detail else f"{reason}: {detail}")


def _github_request(
    url: str,
    token: str,
    *,
    accept: str = "application/vnd.github+json",
) -> tuple[int, bytes, dict[str, str]]:
    req = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": accept,
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "node-webrtc-rust-native-bundle-resolver",
        },
        method="GET",
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            headers = {k.lower(): v for k, v in resp.headers.items()}
            return resp.getcode(), resp.read(), headers
    except urllib.error.HTTPError as exc:
        body = exc.read() if exc.fp else b""
        raise ResolveError(
            "http_error", f"status={exc.code} url={url} body={body[:200]!r}"
        ) from exc
    except urllib.error.URLError as exc:
        raise ResolveError("network_error", str(exc)) from exc


def _parse_json(data: bytes, label: str) -> Any:
    try:
        return json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ResolveError("malformed_api_json", f"{label}: {exc}") from exc


def list_successful_main_runs(
    owner: str,
    repo: str,
    token: str,
    *,
    per_page: int = 30,
    fetch: Callable[..., tuple[int, bytes, dict[str, str]]] | None = None,
) -> list[dict[str, Any]]:
    fetch = fetch or _github_request
    url = (
        f"https://api.github.com/repos/{owner}/{repo}/actions/workflows/"
        f"{nbc.MAIN_WORKFLOW_FILE}/runs?branch=main&status=success&per_page={per_page}"
    )
    _, body, _ = fetch(url, token)
    data = _parse_json(body, "workflow_runs")
    if not isinstance(data, dict) or "workflow_runs" not in data:
        raise ResolveError("malformed_api_json", "missing workflow_runs")
    runs = data["workflow_runs"]
    if not isinstance(runs, list):
        raise ResolveError("malformed_api_json", "workflow_runs not a list")

    trusted: list[dict[str, Any]] = []
    for run in runs:
        if not isinstance(run, dict):
            continue
        path = str(run.get("path") or "")
        name = str(run.get("name") or "")
        head_branch = str(run.get("head_branch") or "")
        conclusion = str(run.get("conclusion") or "")
        status = str(run.get("status") or "")
        if path and not path.endswith(nbc.MAIN_WORKFLOW_FILE):
            continue
        if name and name != nbc.MAIN_WORKFLOW_NAME and "build-main" not in path:
            # Prefer path match; allow name match when path absent in fixtures
            if path and not path.endswith(nbc.MAIN_WORKFLOW_FILE):
                continue
        if head_branch != "main":
            continue
        if status != "completed" or conclusion != "success":
            continue
        if "id" not in run or "head_sha" not in run:
            continue
        trusted.append(run)
    return trusted


def list_run_artifacts(
    owner: str,
    repo: str,
    run_id: int,
    token: str,
    *,
    fetch: Callable[..., tuple[int, bytes, dict[str, str]]] | None = None,
) -> list[dict[str, Any]]:
    fetch = fetch or _github_request
    url = f"https://api.github.com/repos/{owner}/{repo}/actions/runs/{run_id}/artifacts"
    _, body, _ = fetch(url, token)
    data = _parse_json(body, "artifacts")
    if not isinstance(data, dict) or "artifacts" not in data:
        raise ResolveError("malformed_api_json", "missing artifacts")
    arts = data["artifacts"]
    if not isinstance(arts, list):
        raise ResolveError("malformed_api_json", "artifacts not a list")
    return [a for a in arts if isinstance(a, dict)]


def select_bundle_artifact(artifacts: list[dict[str, Any]]) -> dict[str, Any]:
    matches = [
        a
        for a in artifacts
        if a.get("name") == nbc.BUNDLE_ARTIFACT_NAME and a.get("expired") is not True
    ]
    if not matches:
        expired = [
            a
            for a in artifacts
            if a.get("name") == nbc.BUNDLE_ARTIFACT_NAME and a.get("expired") is True
        ]
        if expired:
            raise ResolveError("artifact_expired", nbc.BUNDLE_ARTIFACT_NAME)
        raise ResolveError("artifact_missing", nbc.BUNDLE_ARTIFACT_NAME)
    # Prefer newest by id
    matches.sort(key=lambda a: int(a.get("id") or 0), reverse=True)
    return matches[0]


def download_artifact_zip(
    owner: str,
    repo: str,
    artifact_id: int,
    token: str,
    dest_zip: Path,
    *,
    fetch: Callable[..., tuple[int, bytes, dict[str, str]]] | None = None,
) -> None:
    fetch = fetch or _github_request
    url = (
        f"https://api.github.com/repos/{owner}/{repo}/actions/artifacts/"
        f"{artifact_id}/zip"
    )
    _, body, _ = fetch(url, token, accept="application/vnd.github+json")
    # GitHub may return redirect handled by urlopen; body should be zip bytes
    if len(body) < 4 or body[:2] != b"PK":
        # Some fixtures return JSON error
        try:
            data = json.loads(body.decode("utf-8"))
            raise ResolveError("download_failed", str(data)[:200])
        except (UnicodeDecodeError, json.JSONDecodeError):
            raise ResolveError("download_failed", "response is not a zip archive")
    dest_zip.write_bytes(body)


def extract_bundle(zip_path: Path, dest_dir: Path) -> Path:
    dest_dir.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(zip_path, "r") as zf:
        zf.extractall(dest_dir)
    # Bundle root is either dest_dir itself (meta.json) or a single top folder
    if (dest_dir / "meta.json").is_file():
        return dest_dir
    children = [p for p in dest_dir.iterdir() if p.is_dir()]
    for child in children:
        if (child / "meta.json").is_file():
            return child
    raise ResolveError("invalid_bundle", "meta.json not found after extract")


def order_runs_for_preference(
    runs: list[dict[str, Any]], current_sha: str
) -> list[dict[str, Any]]:
    exact = [r for r in runs if r.get("head_sha") == current_sha]
    others = [r for r in runs if r.get("head_sha") != current_sha]
    # API already returns newest first; keep that order within buckets
    return exact + others


def resolve_native_main_bundle(
    *,
    root: Path,
    owner: str,
    repo: str,
    token: str,
    current_sha: str,
    profile: str,
    download_dir: Path,
    fetch: Callable[..., tuple[int, bytes, dict[str, str]]] | None = None,
    max_runs: int = 30,
) -> dict[str, Any]:
    aggregate = nbc.aggregate_native_digest(root, profile)
    runs = list_successful_main_runs(
        owner, repo, token, per_page=max_runs, fetch=fetch
    )
    if not runs:
        raise ResolveError("no_successful_main_runs")

    ordered = order_runs_for_preference(runs, current_sha)
    last_reason = "no_matching_bundle"
    for run in ordered:
        run_id = int(run["id"])
        head_sha = str(run["head_sha"])
        try:
            arts = list_run_artifacts(owner, repo, run_id, token, fetch=fetch)
            art = select_bundle_artifact(arts)
            zip_path = download_dir / f"bundle-{run_id}.zip"
            download_dir.mkdir(parents=True, exist_ok=True)
            download_artifact_zip(
                owner, repo, int(art["id"]), token, zip_path, fetch=fetch
            )
            extract_root = download_dir / f"extract-{run_id}"
            if extract_root.exists():
                import shutil

                shutil.rmtree(extract_root)
            bundle_dir = extract_bundle(zip_path, extract_root)

            meta = json.loads((bundle_dir / "meta.json").read_text(encoding="utf-8"))
            meta_agg = meta.get("aggregate_digest")
            exact = head_sha == current_sha
            if not exact and meta_agg != aggregate:
                last_reason = "fingerprint_mismatch"
                continue

            # Always validate against *current* workspace contract
            nbc.validate_native_bundle(
                root, bundle_dir, profile, expect_aggregate=aggregate
            )
            return {
                "reused": True,
                "run_id": run_id,
                "artifact_id": int(art["id"]),
                "head_sha": head_sha,
                "exact_sha": exact,
                "aggregate_digest": aggregate,
                "bundle_dir": str(bundle_dir),
                "fallback_reason": "",
                "preference": "exact_sha" if exact else "fingerprint_match",
            }
        except ResolveError as exc:
            last_reason = exc.reason
            continue
        except SystemExit as exc:
            last_reason = f"bundle_validation_failed:{exc}"
            continue

    raise ResolveError(last_reason)


def emit_github_output(result: dict[str, Any]) -> None:
    out = os.environ.get("GITHUB_OUTPUT")
    lines = [
        f"bundle_reused={'true' if result.get('reused') else 'false'}",
        f"run_id={result.get('run_id', '')}",
        f"artifact_id={result.get('artifact_id', '')}",
        f"head_sha={result.get('head_sha', '')}",
        f"exact_sha={'true' if result.get('exact_sha') else 'false'}",
        f"aggregate_digest={result.get('aggregate_digest', '')}",
        f"bundle_dir={result.get('bundle_dir', '')}",
        f"fallback_reason={result.get('fallback_reason', '')}",
        f"preference={result.get('preference', '')}",
    ]
    text = "\n".join(lines) + "\n"
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write(text)
    sys.stdout.write(text)


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--profile", default="release")
    p.add_argument("--download-dir", required=True)
    p.add_argument("--sha", default=os.environ.get("GITHUB_SHA", ""))
    p.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", ""))
    p.add_argument("--token", default=os.environ.get("GITHUB_TOKEN", ""))
    p.add_argument(
        "--fixture-dir",
        default="",
        help="Test-only: load API responses from fixture JSON files",
    )
    args = p.parse_args(argv)

    if not args.repository or "/" not in args.repository:
        print("resolve: GITHUB_REPOSITORY / --repository required", file=sys.stderr)
        return 2
    owner, repo = args.repository.split("/", 1)
    if not args.sha:
        print("resolve: GITHUB_SHA / --sha required", file=sys.stderr)
        return 2

    fetch = None
    if args.fixture_dir:
        fixture = Path(args.fixture_dir)

        def fetch_fixture(
            url: str,
            token: str,
            *,
            accept: str = "application/vnd.github+json",
        ) -> tuple[int, bytes, dict[str, str]]:
            del token, accept
            # Map URL path to fixture file
            parsed = urllib.parse.urlparse(url)
            path = parsed.path
            if path.endswith("/runs") or "workflow_runs" in path or path.endswith(
                f"/workflows/{nbc.MAIN_WORKFLOW_FILE}/runs"
            ):
                data = (fixture / "workflow_runs.json").read_bytes()
                return 200, data, {}
            if "/artifacts/" in path and path.endswith("/zip"):
                # actions/artifacts/{id}/zip
                art_id = path.rstrip("/").split("/")[-2]
                zpath = fixture / f"artifact-{art_id}.zip"
                if not zpath.is_file():
                    raise ResolveError("download_failed", f"missing fixture {zpath}")
                return 200, zpath.read_bytes(), {}
            if path.endswith("/artifacts"):
                run_id = path.rstrip("/").split("/")[-2]
                data = (fixture / f"run-{run_id}-artifacts.json").read_bytes()
                return 200, data, {}
            raise ResolveError("http_error", f"no fixture for {url}")

        fetch = fetch_fixture
        token = args.token or "test-token"
    else:
        token = args.token
        if not token:
            print("resolve: GITHUB_TOKEN required", file=sys.stderr)
            return 2

    root = nbc.repo_root()
    download_dir = Path(args.download_dir).resolve()
    try:
        result = resolve_native_main_bundle(
            root=root,
            owner=owner,
            repo=repo,
            token=token,
            current_sha=args.sha,
            profile=args.profile,
            download_dir=download_dir,
            fetch=fetch,
        )
    except ResolveError as exc:
        result = {
            "reused": False,
            "run_id": "",
            "artifact_id": "",
            "head_sha": "",
            "exact_sha": False,
            "aggregate_digest": nbc.aggregate_native_digest(root, args.profile),
            "bundle_dir": "",
            "fallback_reason": exc.reason,
            "preference": "",
        }
        print(f"resolve: fallback ({exc.reason}) {exc.detail}", file=sys.stderr)
        emit_github_output(result)
        return 0

    emit_github_output(result)
    print(
        f"resolve: reused run={result['run_id']} preference={result['preference']} "
        f"sha={result['head_sha'][:12]}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
