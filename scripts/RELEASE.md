# Release guide

How to publish `@node-webrtc-rust/*` packages to npm — from your machine or via GitHub Actions.

## Packages published

| Package | Description |
| --- | --- |
| `@node-webrtc-rust/bindings` | Main package; downloads platform-specific optional deps |
| `@node-webrtc-rust/bindings-*` | One package per platform (darwin/linux/win `.node` binaries) |
| `@node-webrtc-rust/signaling` | WebSocket signaling helpers |
| `@node-webrtc-rust/sdk` | TypeScript WebRTC + conference + voice API |
| `@node-webrtc-rust/helpers` | Session pod, voice session host, PCM utilities |

Publish order (enforced by all scripts and CI): **platform bindings → bindings → signaling → sdk → helpers**.

---

## One-time setup

### npm organization

Create the scope once: [npmjs.com/org/create](https://www.npmjs.com/org/create) → organization name **`node-webrtc-rust`**.

Your npm user needs publish access to the org.

### Authentication

**Local scripts:** pass a token or use `npm login`.

```bash
export NPM_TOKEN=npm_...   # Automation or Publish token with org access
npm whoami                   # verify
```

**GitHub Actions:** add repository secret **`NPM_TOKEN`** (same token type). The release workflow writes it to `~/.npmrc` before `npm publish`.

Never commit tokens. `release-local.sh` uses a temp `.npmrc_release` file (git-ignored).

### CI Docker image (`ci` branch)

Linux build and test jobs use `ghcr.io/<owner>/node-webrtc-rust/ci-build:latest`.

Pipeline details: [`scripts/ci/README.md`](ci/README.md).

Rebuild the image when `docker/ci/Dockerfile` changes:

1. Merge Dockerfile changes to the **`ci`** branch (or push directly to `ci`).
2. Wait for [CI Docker image](https://github.com/akirilyuk/node-webrtc-rust/actions/workflows/ci-image.yml) to finish.

PR and release workflows always pull `:latest`; they do not rebuild the image.

---

## Changelog workflow

User-facing release notes live in **[`CHANGELOG.md`](../CHANGELOG.md)** at the repo root ( [Keep a Changelog](https://keepachangelog.com/) style).

| When | Action |
| --- | --- |
| **During development** | Add bullets under `[Unreleased]` as PRs merge |
| **Before tagging** | Rename `[Unreleased]` → `[X.Y.Z] — YYYY-MM-DD`, add empty `[Unreleased]` at top, commit on `main` |
| **On tag push** | CI publishes npm; GitHub Release body is extracted from the `[X.Y.Z]` section via [`scripts/changelog-release-body.sh`](changelog-release-body.sh) |
| **After publish** | Commit version bumps on `main` (`chore(repo): release X.Y.Z`) |

Preview release notes locally:

```bash
bash scripts/changelog-release-body.sh 0.3.0
```

---

## Release via GitHub Actions (recommended)

Use this for **all six platform binaries** and a consistent CI run before publish.

### Branch workflow (prepare)

1. Merge your changes to **`main`** via PR (Build & Test runs on the PR).
2. Optionally bump versions in git on `main` — the release workflow sets npm versions from the tag, so a version commit is not required before tagging.

### Tag workflow (publish)

1. Finalize [`CHANGELOG.md`](../CHANGELOG.md) for the version (see [Changelog workflow](#changelog-workflow)).
2. Merge to **`main`** and confirm Build & Test is green.
3. Push a tag matching `release/<semver>`:

```bash
git checkout main
git pull
git tag release/0.3.0
git push origin release/0.3.0
```

Supported tag forms:

```text
release/0.2.0
release/0.2.0-beta.1
release/0.2.0-rc.1
```

The segment after `release/` becomes the npm version for all packages.

### What the workflow does

[`.github/workflows/release.yml`](../.github/workflows/release.yml):

1. **Build** — full matrix (3× Linux in CI container, macOS ×2, Windows ×1)
2. **Test** — format, lint, typecheck, `cargo test`, `npm test` (with coturn)
3. **Publish** — stage artifacts, bump versions, `napi prepublish`, publish to npm (including `@node-webrtc-rust/helpers`)
4. **GitHub Release** — creates a release with the matching section from `CHANGELOG.md`

Required secrets: **`NPM_TOKEN`**, **`GITHUB_TOKEN`** (provided by Actions for the release step).

### After a CI release

Commit version bumps on `main` if you want the repo to match npm (optional but recommended):

```bash
git pull
# versions were set in CI; pull or re-run release-local.sh --dry-run to sync locally
git add -A
git commit -m "chore(repo): release 0.2.0"
git push origin main
```

---

## Local release

Two scripts live under `scripts/`. Pick based on how many platforms you can build locally.

### Quick release — current platform only

[`release-local.sh`](release-local.sh) — best for **patch releases** when only your host `.node` matters, or for testing the publish flow.

```bash
./scripts/release-local.sh <version> <npm-token> [--dry-run] [--otp=CODE]
```

Examples:

```bash
./scripts/release-local.sh 0.2.0 "$NPM_TOKEN" --dry-run
./scripts/release-local.sh 0.2.0 "$NPM_TOKEN" --otp=123456
```

Behavior:

- Detects host OS/arch and looks for a matching `.node` in `packages/bindings/`, `prebuilt/`, or `artifacts/`
- Rebuilds with `npm run build:local` only if missing
- Bumps all package versions via direct JSON edits (no registry lookups)
- Publishes **only platform packages that have a `.node` file** (typically one on a dev machine)
- Verifies the native binding loads before publish

After a successful publish:

```bash
git add -A && git commit -m "chore(repo): release 0.2.0"
git tag release/0.2.0 && git push origin release/0.2.0   # optional: record on GitHub
```

### Full release — all six platforms (macOS)

[`release-publish.sh`](release-publish.sh) — builds Linux + macOS on a Mac; **Windows `.node` must be supplied** (CI artifact or manual build).

```bash
export NPM_TOKEN=npm_...
./scripts/release-publish.sh <version> [--dry-run] [--force-build] [--otp=CODE]
# or: npm run release:publish -- 0.2.0
```

Prerequisites on macOS: Docker (optional for Linux-only CI verify), `cmake`, `zig`, Rust stable with cross targets.

Place prebuilt binaries under any of:

- `packages/bindings/node-webrtc-rust.<platform>.node` (see `index.js` naming)
- `packages/bindings/prebuilt/bindings-<rust-triple>/`
- `packages/bindings/artifacts/bindings-<rust-triple>/` (populated by the script)

Windows is never cross-compiled locally — copy from a Windows CI artifact or build on Windows, then re-run.

---

## Pre-release checks

Mirror PR CI locally:

```bash
npm run ci:verify              # Linux native builds + full test suite in Docker
npm run ci:verify:linux        # native matrix only
npm run ci:verify:checks:docker
```

Dry-run publish packaging on a PR: the **Publish (dry-run)** job in [Build & Test](../.github/workflows/build.yml) runs `npm publish --dry-run --workspaces` when native or TS code changes.

---

## Troubleshooting

| Issue | Fix |
| --- | --- |
| `shopt: not found` in CI | Fixed — Linux container steps use `shell: bash` |
| `EOTP` / 2FA on npm | Re-run local script with `--otp=123456` |
| `403` on scoped publish | Use `npm publish --access public` (scripts do this) |
| Double publish of platform pkgs | Publish main bindings with `--ignore-scripts` (scripts do this) |
| Missing Windows binary locally | Use CI release tag workflow or add `node-webrtc-rust.win32-x64-msvc.node` |
| Zig / Opus link errors on Linux | Set `OPUS_STATIC=1` and `CMAKE_POLICY_VERSION_MINIMUM=3.5` (CI and scripts set these) |
