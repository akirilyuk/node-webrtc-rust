# Release guide

How to publish `@node-webrtc-rust/*` packages to npm — from your machine or via GitHub Actions.

## Packages published

| Package                        | Description                                                  |
| ------------------------------ | ------------------------------------------------------------ |
| `@node-webrtc-rust/bindings`   | Main package; downloads platform-specific optional deps      |
| `@node-webrtc-rust/bindings-*` | One package per platform (darwin/linux/win `.node` binaries) |
| `@node-webrtc-rust/signaling`  | WebSocket signaling helpers                                  |
| `@node-webrtc-rust/sdk`        | TypeScript WebRTC + conference + voice API                   |
| `@node-webrtc-rust/helpers`    | Session pod, voice session host, PCM utilities               |

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

## Package versions in git vs npm

Two places versions matter:

| Where | When bumped | Committed to git? |
| ----- | ----------- | ----------------- |
| **npm registry** | In CI/local **immediately before** `npm publish` | No |
| **git (`package.json`)** | On the **release prep PR**, **before** the tag | **Yes — required** |

CI [`release.yml`](../.github/workflows/release.yml) always rewrites versions in the publish job workspace from the tag (`release/X.Y.Z` → `X.Y.Z`), then publishes. That ephemeral bump does **not** update `main`. If git is never bumped, the repo drifts (e.g. npm at `0.4.0`, git still at `0.1.5`).

**Rule:** committed `package.json` versions must match the version you are about to tag **before** `git push origin release/X.Y.Z`. Do **not** rely on a post-publish commit on `main` (easy to forget; no `chore(repo): release` commits have been made so far).

**Release prep** (before platform packages exist on npm) — bump `package.json` only:

```bash
SKIP_LOCK_REFRESH=1 bash scripts/ci/bump-workspace-versions.sh 0.4.0
```

**After npm publish** — lockfile is refreshed automatically (see below). Manual:

```bash
bash scripts/ci/post-release-sync-main-package-lock.sh 0.4.0
```

Do **not** hand-edit `package-lock.json` optional `@node-webrtc-rust/bindings-*` entries — stubs break `npm ci` with `Invalid Version:`.

GitHub Actions runs **`validate-package-lock`** on every PR / `main` / release tag. Local: `npm run ci:validate:package-lock`.

CI publish still runs `npm version` + `napi version` + [`set-release-deps.sh`](ci/set-release-deps.sh) before `npm publish`; release prep should already match the tag version in git.

---

## Changelog workflow

User-facing release notes live in **[`CHANGELOG.md`](../CHANGELOG.md)** at the repo root ( [Keep a Changelog](https://keepachangelog.com/) style).

| When                   | Action                                                                                                                                             |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| **During development** | Add bullets under `[Unreleased]` as PRs merge                                                                                                      |
| **Release prep PR**    | On branch `release/X.Y.Z`: finalize `[X.Y.Z]` section, bump all `package.json` to `X.Y.Z`, open PR → `main`                                         |
| **On tag push**        | CI publishes npm; GitHub Release body from [`scripts/changelog-release-body.sh`](changelog-release-body.sh)                                        |

Preview release notes locally:

```bash
bash scripts/changelog-release-body.sh 0.3.0
```

---

## Release via GitHub Actions (recommended)

Use this for **all six platform binaries** and a consistent CI run before publish.

### 1. Prepare release (PR — do not commit directly on `main`)

1. Feature work is already merged to **`main`** via normal PRs.
2. Create branch **`release/X.Y.Z`** from `main` (e.g. `release/0.4.1`).
3. On that branch:
   - Finalize [`CHANGELOG.md`](../CHANGELOG.md): `[Unreleased]` → `[X.Y.Z] — YYYY-MM-DD`, new empty `[Unreleased]`.
   - Bump **all** publishable package versions and internal pins to `X.Y.Z` with `SKIP_LOCK_REFRESH=1` (see [Package versions in git vs npm](#package-versions-in-git-vs-npm)). Do **not** commit a broken `package-lock.json` from a pre-publish refresh.
4. Open PR **`release/X.Y.Z` → `main`**, wait for Build & Test green, **merge**.

### 2. Tag and publish (after merge)

Tag the **merge commit** on `main` (versions in tree must already be `X.Y.Z`):

```bash
git checkout main
git pull
git tag release/0.4.1
git push origin release/0.4.1
```

Supported tag forms:

```text
release/0.2.0
release/0.2.0-beta.1
release/0.2.0-rc.1
```

The segment after `release/` must match committed `package.json` versions. CI uses the tag for publish metadata and re-applies the same bump in the runner before `npm publish`.

### What the workflow does

[`.github/workflows/release.yml`](../.github/workflows/release.yml):

1. **Build** — full matrix (3× Linux in CI container, macOS ×2, Windows ×1)
2. **Test** — format, lint, typecheck, `cargo test`, `npm test` (with coturn)
3. **Publish** — stage artifacts, bump versions in workspace, build TS, publish to npm (including `@node-webrtc-rust/helpers`)
4. **GitHub Release** — creates a release with the matching section from `CHANGELOG.md`
5. **Sync main package-lock** — checks out `main`, runs [`post-release-sync-main-package-lock.sh`](ci/post-release-sync-main-package-lock.sh), opens PR `chore/post-release-package-lock-X.Y.Z` → `main` (merge when green)

Required secrets: **`NPM_TOKEN`**, **`GITHUB_TOKEN`** (publish, release, and automated PR).

Release prep PR bumps git `package.json` versions; the **post-release PR** updates `package-lock.json` from npm so `main` stays valid for `npm ci`. Merge that PR promptly after each tag.

### Catch-up: repo behind npm (e.g. after 0.4.0 without a version PR)

If npm already has `X.Y.Z` but git does not:

1. Branch from `main` (e.g. `chore/sync-versions-0.4.0` or `release/sync-0.4.0`).
2. Bump git to match npm (`0.4.0`) using the commands above; **no** new tag.
3. PR → `main`, merge.
4. Then run the normal [prepare release](#1-prepare-release-pr--do-not-commit-directly-on-main) flow for the next version (`0.4.1`, etc.).

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

After a successful publish (local only — versions should already be committed on `release/X.Y.Z` before tagging):

```bash
# If you bumped only in the working tree during a local publish:
git add -A && git commit -m "chore(repo): release 0.2.0"
git tag release/0.2.0 && git push origin release/0.2.0
```

Prefer the [GitHub Actions](#release-via-github-actions-recommended) PR + tag flow so git and npm stay aligned without a post-publish commit.

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
npm run build:native             # host .node for npm test
npm run ci:verify                # full PR check suite on host
npm run ci:verify:release-ts     # release publish TS path
npm run ci:verify:linux          # optional: native matrix cross-builds in Docker
```

Dry-run publish packaging on a PR: the **Publish (dry-run)** job in [Build & Test](../.github/workflows/build.yml) runs `npm publish --dry-run --workspaces` when native or TS code changes.

---

## Troubleshooting

| Issue                           | Fix                                                                                   |
| ------------------------------- | ------------------------------------------------------------------------------------- |
| `shopt: not found` in CI        | Fixed — Linux container steps use `shell: bash`                                       |
| `EOTP` / 2FA on npm             | Re-run local script with `--otp=123456`                                               |
| `403` on scoped publish         | Use `npm publish --access public` (scripts do this)                                   |
| Double publish of platform pkgs | Publish main bindings with `--ignore-scripts` (scripts do this)                       |
| Missing Windows binary locally  | Use CI release tag workflow or add `node-webrtc-rust.win32-x64-msvc.node`             |
| Zig / Opus link errors on Linux | Set `OPUS_STATIC=1` and `CMAKE_POLICY_VERSION_MINIMUM=3.5` (CI and scripts set these) |
