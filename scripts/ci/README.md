# CI pipelines

Human-readable reference for GitHub Actions workflows, reusable jobs, caches, and local mirrors.

**When you change anything under `.github/`, `scripts/ci/`, or `docker/ci/`**, update this file in the same PR.

---

## Overview

| Workflow                | File                                                                                         | Trigger                              | Purpose                                                               |
| ----------------------- | -------------------------------------------------------------------------------------------- | ------------------------------------ | --------------------------------------------------------------------- |
| **Build & Test (PR)**   | [`.github/workflows/build.yml`](../../.github/workflows/build.yml)                           | PR → `main`, `workflow_dispatch`     | Path-filtered quality, native compile, TS build, integration tests    |
| **Build & Test (main)** | [`.github/workflows/build-main.yml`](../../.github/workflows/build-main.yml)                 | Push → `main`                        | Full release native matrix + full test suite + `native-main-bundle`   |
| **Release**             | [`.github/workflows/release.yml`](../../.github/workflows/release.yml)                       | Tag `release/*`                      | Resolve/reuse main bundle → tests → npm publish → GitHub Release      |
| **CI Docker image**     | [`.github/workflows/ci-image.yml`](../../.github/workflows/ci-image.yml)                     | Push → `ci`/`main` (paths), dispatch | Publish `ci-build` + `ci-build-alpine` (`:latest` + immutable `:SHA`) |
| **Native cache smoke**  | [`.github/workflows/native-cache-smoke.yml`](../../.github/workflows/native-cache-smoke.yml) | **`workflow_dispatch` only**         | Non-publishing cache reuse + release-style resolve (see below)        |

Every PR/main/release workflow starts with **`validate-package-lock`** (no path filter) — fails in seconds if `package-lock.json` has stub optional `@node-webrtc-rust/bindings-*` entries. Local: `npm run ci:validate:package-lock`.

`build.yml` also accepts `workflow_dispatch`: [`release.yml`](../../.github/workflows/release.yml) job `sync-main-package-lock` dispatches it on the post-release lockfile PR branch (GITHUB_TOKEN-created PRs get no `pull_request` runs) and enables squash auto-merge. Under dispatch, `Detect changes` diffs `origin/main` against the branch tip; check names are unchanged so the `main` ruleset is satisfied. See [`RELEASE.md`](../RELEASE.md#post-release-pr-automation).

Reusable workflows (called via `workflow_call`, not triggered directly):

| File                                                                           | Role                                                           |
| ------------------------------------------------------------------------------ | -------------------------------------------------------------- |
| [`reusable-build-linux.yml`](../../.github/workflows/reusable-build-linux.yml) | Linux release matrix (gnu, musl, arm64)                        |
| [`reusable-build-host.yml`](../../.github/workflows/reusable-build-host.yml)   | macOS + Windows release matrix                                 |
| [`reusable-test.yml`](../../.github/workflows/reusable-test.yml)               | Download binding artifact → cache fallback → host Docker tests |

Composite actions live in [`.github/actions/`](../../.github/actions/).

## Concurrency (cancel in-progress)

PR and main build workflows use GitHub `concurrency` with `cancel-in-progress: true` so a new push to the same PR (or to `main`) cancels the previous in-flight run and starts a fresh one with the updated commit.

| Workflow                | Group key              | Cancel in progress             |
| ----------------------- | ---------------------- | ------------------------------ |
| **Build & Test (PR)**   | `build-pr-<PR number>` | yes                            |
| **Build & Test (main)** | `build-main-<ref>`     | yes                            |
| **CI Docker image**     | `ci-image-<ref>`       | yes                            |
| **Release**             | _(none)_               | no — do not cancel mid-publish |

## Runners

| Platform               | `runs-on`                                  | Workflows                                                                                          |
| ---------------------- | ------------------------------------------ | -------------------------------------------------------------------------------------------------- |
| Linux x64 (gnu + musl) | `self-hosted` + `ci-build` container       | Linux x64 release matrix, integration tests (`ci-test` container), CI image build, release publish |
| Linux arm64 (gnu)      | `ubuntu-24.04-arm` (GitHub-hosted, native) | Linux release matrix only                                                                          |
| macOS                  | `macos-latest`                             | Release host matrix (darwin x64 + arm64)                                                           |
| Windows                | `windows-latest`                           | Release host matrix (x64)                                                                          |

**Linux gnu x64** builds natively on the self-hosted runner without `napi --zig` so Sherpa/ONNX static objects link correctly (`__cpu_features2`). **Linux x64 musl** builds natively in **`ghcr.io/.../ci-build-alpine:latest`** with **musl Sherpa shared libs** (`build-sherpa-onnx-musl-libs.sh` + Alpine `onnxruntime-dev`); `vendor-sherpa-onnx` selects `sherpa-onnx/shared` via `target_env = "musl"` — the default `sherpa-onnx-sys` glibc static prebuilts fail on Alpine (`__strdup: symbol not found`). CI sets `SHERPA_ONNX_LIB_DIR=/opt/sherpa-musl/lib` **only on the musl job** (gnu/arm64 must not export an empty value). CI runs `verify-musl-runtime.sh` after musl builds. **Linux arm64 gnu** builds on GitHub-hosted ARM runners (native compile, no Zig cross) to avoid build-script `ring` arch mismatches.

The self-hosted runner must have **Docker** (runner user in the `docker` group). Container jobs run as root and leave root-owned files; host jobs run an inline **Docker `chown`** prepare step before checkout (no passwordless sudo required).

---

## PR pipeline (`build.yml`)

```mermaid
flowchart TD
  changes[Detect changes]
  quality[Typecheck and lint]
  ts[Build TypeScript]
  integ["Integration tests (ci-test container)"]
  test[Test]

  changes --> quality
  changes --> ts
  ts --> integ
  quality --> test
  ts --> test
  integ --> test
```

### 1. Detect changes

[`compute-pr-job-gates.sh "$PR_BASE_SHA" "$PR_HEAD_SHA"`](compute-pr-job-gates.sh) sets `run_compile` / `run_build_ts` / `run_test` / `run_quality` from:

1. **`merge-base..pull_request.head.sha`** (full PR diff — not `github.sha`, which is a synthetic merge commit)
2. **Files in the latest head commit** (`git diff-tree`) so a push that only edits `crates/speech/**` still forces the integration tests (which build the native binding)

There is no path-filter action. One run per branch: the concurrency group is keyed on the head repo + branch, so a dispatched run and a pull_request run of the same branch collapse. Rules:

- **Docs only** (`*.md`, `docs/**`, `.cursor/**`, `LICENSE*`): quality only (Prettier covers Markdown). No build-ts or integration tests.
- **Everything else**: full pipeline (build-ts + integration + quality). Integration builds its own debug `.node`, so `run_test` implies `run_compile` in the gate output.
- **Version-only** (`package.json` / `package-lock.json` diffs that only change our own `@node-webrtc-rust/*` versions, checked by [`version-only-diff.py`](version-only-diff.py)): quality + build-ts only. Any third-party version change, new dependency, other file, or invalid JSON runs the full pipeline. `main` still runs the full suite after merge.

The script also prints `version_only=true|false`.

Local check (hermetic, builds its own temp git repos): `bash scripts/ci/compute-pr-job-gates.test.sh`

### 2. Package-lock optional bindings (always)

- **When:** every PR (always runs; not path-filtered)
- **Job:** `validate-package-lock` → [`.github/actions/validate-package-lock`](../../.github/actions/validate-package-lock/action.yml)
- **Script:** [`validate-package-lock-optional-bindings.sh`](validate-package-lock-optional-bindings.sh) — no `npm ci`; blocks merge before opaque `Invalid Version:` errors

### 3. Typecheck & lint

- **Always runs** on every PR (for branch-protection required checks).
- **When:** `run_quality` (every diff, including docs-only PRs, which run quality only) — otherwise the job succeeds immediately after a skip notice (no checkout or setup-node).
- **When running:** [`run-pr-quality.sh`](run-pr-quality.sh) on self-hosted runner.
- **Runner:** `self-hosted` + `actions/setup-node@v20` (not `ci-build` — fast, no GHCR pull)
- **Script:** [`run-pr-quality.sh`](run-pr-quality.sh) → [`validate-package-lock-optional-bindings.sh`](validate-package-lock-optional-bindings.sh), `npm ci`, `fix-rollup-native.sh`, typecheck ([`tsconfig.typecheck.json`](tsconfig.typecheck.json)), `eslint`, helpers vitest, [`run-sherpa-example-ci.sh typecheck`](run-sherpa-example-ci.sh)
- Runs [`build-ts-workspace.sh`](build-ts-workspace.sh) inside [`run-helpers-unit-tests.sh`](run-helpers-unit-tests.sh) when sdk/signaling/helpers `dist/` is missing (fresh CI checkout). Job 4 still builds once for Test cache.

`Build TypeScript` and the integration job do not wait for this job; the `Test` gate requires it (see [Test](#6-test)). A lint failure therefore still blocks merge, but does not delay the build or the tests.

### 4. Native binding (no separate job on PRs)

There is no `Compile native` job on PRs. The integration job builds the debug `x86_64-unknown-linux-gnu` binding itself when `packages/bindings/*.node` is missing: [`run-pr-integration.sh`](run-pr-integration.sh) runs [`build-native-addon.sh`](build-native-addon.sh) (`--profile debug`, which also runs `copy:local-node`) under `ci-step` with a 1200 s cap (`CI_NATIVE_BUILD_TIMEOUT_SEC`). Main and release still build release bindings in their own jobs and pass the artifact in, so the script uses it as is. Cache keys for release bindings use `native-v3-{profile}-{target}-{input_digest}` from [`native-build-fingerprint.sh`](native-build-fingerprint.sh) / [`native_build_contract.py`](native_build_contract.py) (see [Native fingerprint & provenance](#native-fingerprint--provenance-foundation)).

### 5. Build TypeScript

- **Always runs** on every PR (for branch-protection required checks).
- **When:** `run_build_ts` — otherwise the job succeeds immediately after a skip notice (no checkout, setup-node, or cache).
- **Needs:** package-lock validation only. It starts without waiting for **Typecheck & lint**.
- **Runner:** `self-hosted` + `setup-node`
- **Cache:** [`ci-cache-ts-dist`](../../.github/actions/ci-cache-ts-dist) → `packages/sdk/dist`, `packages/signaling/dist`, `packages/helpers/dist`
- **On cache miss:** `npm ci`, `fix-rollup-native.sh`, [`build-ts-workspace.sh`](build-ts-workspace.sh) (sdk core → signaling → full sdk → helpers)

Single CI build of publishable `dist/` for the Test job. Release-publish compile parity: [`verify-release-publish-ts.sh`](verify-release-publish-ts.sh) locally or `release.yml` publish job.

### 6. Test

- **Always runs** on every PR (for branch-protection required checks).
- **Required check name:** a thin `Test` job in [`build.yml`](../../.github/workflows/build.yml) gates on the reusable workflow. `workflow_call` jobs alone report as `Integration tests / Test`, which does **not** satisfy the required `Test` context. The gate requires `Typecheck & lint` = success, `Build TypeScript` = success or skipped, and `Integration tests` = success.
- **When:** tests are not skipped (`skip_test` false) — otherwise the integration job succeeds immediately (`skip: 'true'`), e.g. docs-only and version-only PRs.
- **Workflow:** [`reusable-test.yml`](../../.github/workflows/reusable-test.yml) (called as **Integration tests**), runs in the `ci-test` container (`container:` job, no `docker` calls in steps)
- **Script:** [`run-pr-integration.sh`](run-pr-integration.sh)

Inside the job:

1. **Native binding:** PRs build the debug `.node` in the job (see above). Main and release pass the release artifact (`binding_artifact`), which the workflow stages into `packages/bindings/`.
2. **TURN:** with `TURN_AVAILABLE=1` the script starts coturn via [`turnserver-ci.sh`](turnserver-ci.sh) (`start`, bounded 10 s wait; a trap runs `stop` on exit). If something already listens on `127.0.0.1:3478` it is reused.
3. **TS `dist/`** via [`ci-cache-ts-dist`](../../.github/actions/ci-cache-ts-dist) (`ts_dist_profile: pr` on PR builds).
4. **Sherpa models** come from the image (`NWR_CI_SHERPA_MODELS=/opt/sherpa-models`, linked by `ensure-sherpa-models.sh`); there is no model cache step.

The job runs as root in the container and checkout cleans the workspace. **`cargo test`** compiles Rust **test** deps in a separate profile from the NAPI addon; there is no `target/` handoff between jobs. At the end the script prints `du -sh target third_party/*/target` to size a future build cache.

Local contract checks: `bash scripts/ci/native-build-fingerprint.test.sh`, `bash scripts/ci/native-artifact-manifest.test.sh`, `bash scripts/ci/check-release-targets.test.sh`, `bash scripts/ci/native-binding-cache-key.test.sh`

**TURN test networking:** peers and coturn run in the same job container, so traffic stays on loopback. coturn uses UDP/TCP **3478** for TURN control and **49152–65535** for relay allocations inside the container only. CI enables `--allow-loopback-peers` because both WebRTC peers run on the same host. The `pr-workflow-gates.test.sh` guard keeps `TURN_AVAILABLE: '1'` in the workflow so `packages/sdk/tests/turn.test.ts` cannot silently skip.

### Path-based test suites (PRs)

The integration job runs three suites, each with its own cost: **rust** (Rust fmt + clippy and every `cargo test` step, including the vendored `dtls` and `webrtc-ice` crates and the bench check), **node** (`npm test`, the native exports check, helpers native integration; needs TURN) and **sherpa** ([`run-sherpa-example-ci.sh e2e`](run-sherpa-example-ci.sh)). Node and sherpa need the debug `.node` and the TS `dist/`; rust needs neither.

On PRs, [`compute-pr-job-gates.sh`](compute-pr-job-gates.sh) emits `test_suites` (a comma list in the order `rust,node,sherpa`, empty when no tests run). `build.yml` passes it to [`reusable-test.yml`](../../.github/workflows/reusable-test.yml), which exports it as `CI_TEST_SUITES` for [`run-pr-integration.sh`](run-pr-integration.sh).

- The map is [`pr-test-suites.map`](pr-test-suites.map): one rule per line, `<bash-glob> <suites>`. The first matching rule wins and `*` matches `/`. Docs paths are ignored; a changed path with no rule runs all three suites. A malformed line fails the gate script (exit 1).
- `build-main.yml`, `release.yml`, local runs and the release-tag local gate never set `CI_TEST_SUITES`, so they always run everything.
- A new crate, package, example or vendored directory needs a `<dir>/*` line in the map. [`pr-test-suites-map.test.sh`](pr-test-suites-map.test.sh) fails otherwise, and also fails on a rule that matches no tracked file.

### Sherpa roundtrip E2E (integration job)

After `cargo test`, `npm test`, and helpers `test:integration`, [`run-pr-integration.sh`](run-pr-integration.sh) runs [`run-sherpa-example-ci.sh e2e`](run-sherpa-example-ci.sh):

1. Link the STT (`sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06`) and TTS (`vits-piper-en_US-amy-low`) models from the image into `examples/voice-agent-local-sherpa/.models/` (downloaded locally if missing)
2. Set `SHERPA_STT_MODEL_PATH` / `SHERPA_TTS_MODEL_PATH`
3. Run ignored Rust vendor tests that need those weights: `cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_cancel_not_cached_test -- --ignored` (asserts a cancelled synthesis is not cached). Timeout **420s** (`CI_SHERPA_RUST_IGNORED_TIMEOUT_SEC`). Models-only path: `bash scripts/ci/run-sherpa-example-ci.sh rust`.
4. Run each `start:roundtrip*` script in order (exit on first failure). Each step uses [`run-sherpa-roundtrip-e2e.sh`](run-sherpa-roundtrip-e2e.sh): streams **`[speech]` events** (browser parity) with **`[voice-debug]` / topology off**; **automatic re-run with `VOICE_DEBUG=1`** if that pass fails. Wrapped in [`run-with-timeout.sh`](run-with-timeout.sh) (default **180s** per script; override `CI_SHERPA_ROUNDTRIP_TIMEOUT_SEC`). Model downloads cap at **900s** (`CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC`). The re-run only gets the time left under the step cap (`SHERPA_ROUNDTRIP_STEP_TIMEOUT_SEC` -> `SHERPA_ROUNDTRIP_WALL_MAX_MS`) and is skipped when under 45s is left. A script's in-process wall-clock limit must fit in its step cap; `examples/voice-agent-local-sherpa/src/roundtrip-ci-step-budget.test.ts` (part of `run-sherpa-example-ci.sh vitest`) fails otherwise. `run-sherpa-example-ci.sh step-timeouts` prints every cap.

| Quality job ([`run-pr-quality.sh`](run-pr-quality.sh))                              | Integration job ([`run-pr-integration.sh`](run-pr-integration.sh)) |
| ----------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| Sherpa example `tsc`                                                                | Same models + native `.node` as browser demo                       |
| Vitest: `npm run test:roundtrip-counting` (evaluators only — **no Sherpa weights**) | Full E2E below                                                     |

| #   | npm script                                | Purpose                                                  |
| --- | ----------------------------------------- | -------------------------------------------------------- |
| 1   | `start:roundtrip-counting`                | One long count 1–20 → **1×** `user_speech_final`         |
| 2   | `start:roundtrip-utterance-timing`        | `user_speaking_end` → `user_speech_final` within 500 ms  |
| 3   | `start:roundtrip-two-phrases`             | Two phrases → **2×** finals (multi-turn)                 |
| 4   | `start:roundtrip-barge-in`                | Semantic barge-in (streaming TTS; tone vs spoken)        |
| 5   | `start:roundtrip-barge-in-buffered`       | Same barge-in harness with `VOICE_TTS_STREAM_CHUNKS=0`   |
| 6   | `start:roundtrip-counting-echo`           | Agent1↔Agent2 “You said” echo (counting + long sentence) |
| 7   | `start:roundtrip-counting-barge-recovery` | Full echo → barge truncate → recovery                    |
| 8   | `start:roundtrip`                         | Five default phrases + word similarity                   |

**Local mirror (models + `npm run build:native` first):**

```bash
cd node-webrtc-rust
bash scripts/ci/run-sherpa-example-ci.sh vitest   # quality parity, no models
bash scripts/ci/run-sherpa-example-ci.sh rust     # download models + TTS phrase-cache ignored cargo tests
bash scripts/ci/run-sherpa-example-ci.sh e2e      # models + phrase-cache tests + all roundtrip E2E scripts
bash scripts/ci/run-pr-tests-full.sh              # quality + integration (full PR test job)
```

**Local mirror of the PR Test job (host — recommended):**

```bash
cd node-webrtc-rust
npm run build:native                           # host .node for npm test
npm run ci:verify:pr-full                      # quality + integration
npm run ci:verify:checks                       # full suite incl. format + release TS parity
CI_STEP_LOG_TS=1 npm run ci:verify:pr-full     # UTC timestamps on [ci-step] lines
```

**Optional Docker parity** (`ci-test` image, coturn started in-container — only when debugging remote runner differences):

```bash
cd node-webrtc-rust
npm run ci:verify:pr-test:docker              # integration only (cargo + npm test + Sherpa E2E)
npm run ci:verify:pr-full:docker              # quality + integration
CI_STEP_LOG_TS=1 npm run ci:verify:pr-test:docker   # UTC timestamps on [ci-step] lines
```

Step banners: `[ci-step] START (3/7) sherpa e2e start:roundtrip-barge-in` → `OK (25s)` or `FAIL` / timeout hint.  
A step that passes but used at least 75 % of its `--timeout` also prints `[ci-step] WARN <label> used <d>s of <cap>s (<pct>%)` (plus a GitHub `::warning` and a `$GITHUB_STEP_SUMMARY` line); `CI_STEP_WARN_PCT` changes the threshold. The integration job compiles all Rust test binaries in a separate `cargo test build` step (`CI_CARGO_BUILD_TIMEOUT_SEC`, default 900), so the per-crate caps (`CI_CARGO_LIB_TIMEOUT_SEC`, 180) measure test time only; `run-pr-integration.test.sh` keeps that step in lockstep with the cargo steps.  
Sherpa stderr during E2E: `[topology]` (signaling / agent-pc / user-pc attach), `[e2e-phase]`, `[speech]` (listener events), `[voice-debug]` (Rust STT/VAD). See [`ROUNDTRIP.md`](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md) § Debug logging.

**Pre-push (scoped):** `npm run ci:pre-push` runs Sherpa typecheck + Vitest + E2E when `examples/voice-agent-local-sherpa/` or `crates/speech/` changed — see [`run-pre-push-gates.sh`](run-pre-push-gates.sh).

Details, env vars, and debug logging: [`examples/voice-agent-local-sherpa/ROUNDTRIP.md`](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md).

---

## Main push pipeline (`build-main.yml`)

Triggered on every push to `main`.

```mermaid
flowchart TD
  quality[Typecheck and lint]
  plan[Plan native builds per cache hash]
  buildLinux[build-linux partial matrix]
  buildHost[build-host partial matrix]
  stage[stage-cached-bindings]
  test[Integration tests]

  quality --> plan
  plan --> buildLinux
  plan --> buildHost
  plan --> stage
  buildLinux --> test
  buildHost --> test
  stage --> test
  quality --> test
```

1. **quality** — [`run-pr-quality.sh`](run-pr-quality.sh)
2. **plan** — [`plan-native-builds`](../../.github/actions/plan-native-builds) runs `cargo metadata` inside `CI_IMAGE` on bare self-hosted runners (`CARGO_VIA_CI_IMAGE=1`; no host `rustup`), then probes exact `native-v3-release-{target}-{digest}` keys (curl + equality); matrices only for misses
3. **build-linux / build-host** — compile misses; **always save** per-target `.node`+manifest after compile; upload `bindings-<triple>/`
4. **stage-cached** — restore exact cache hits, refresh manifests, upload same artifact layout
5. **test** — [`run-pr-integration.sh`](run-pr-integration.sh)
6. **assemble-native-bundle** — after Test success, validate all six targets into workflow artifact `native-main-bundle` (90-day retention)

No path filtering — always validates release surface after merge, but skips compile for warm per-target caches.

---

## Release pipeline (`release.yml`)

Prep PRs use branch `release-prep/x.y.z` → `main`. Publish is triggered by `git push origin refs/tags/release/x.y.z`. Full release + lockfile docs: [`scripts/RELEASE.md`](../RELEASE.md#package-lockjson-after-release).

Publish is sequential (platform bindings → bindings → signaling → sdk → helpers). Each step uses [`publish-npm-if-needed.sh`](publish-npm-if-needed.sh) (skip if `pkg@version` is already on the registry) then [`wait-for-npm-package.sh`](wait-for-npm-package.sh). The wait uses **exponential backoff** (3s, 6s, 12s, … capped at 60s; 35 attempts, ~30 min; also checks `GET registry.npmjs.org/{pkg}/{version}`). Platform binding packages use deferred verify (`NPM_PUBLISH_DEFER_VERIFY=1`) and are drained by `wait-for-pending-npm-packages.sh` before `bindings` is published; see `RELEASE.md` "Partial publish / registry lag". A slow `npm view` after `sdk` must not leave `helpers` unpublished on retry.

Resume a mid-sequence publish without retagging: Actions → **Publish npm package if needed** (`publish-npm-if-needed.yml`) → `helpers` / `0.8.1` on `main`. The workflow needs `packages: read` so the job can pull `ghcr.io/<repo>/ci-build`.

```mermaid
flowchart TD
  lock[validate-package-lock always]
  quality[Typecheck and lint]
  plan[Plan + check main CI]
  buildLinux[build-linux if not all cached]
  buildHost[build-host if not all cached]
  stage[stage-cached-bindings]
  test[Integration tests unless main validated + all cached]
  publish[Publish npm + GitHub Release]
  sync[sync-main-package-lock PR to main]

  lock --> quality
  quality --> plan
  plan --> buildLinux
  plan --> buildHost
  plan --> stage
  buildLinux --> test
  buildHost --> test
  stage --> test
  buildLinux --> publish
  buildHost --> publish
  stage --> publish
  test --> publish
  quality --> publish
  publish --> sync
```

1. **validate-package-lock** — always (no path filter); [`validate-package-lock-optional-bindings.sh`](validate-package-lock-optional-bindings.sh)
2. **quality** — [`run-pr-quality.sh`](run-pr-quality.sh)
3. **plan** — resolve `native-main-bundle` from successful main (exact SHA → fingerprint match); else per-target cache plan; [`check-main-ci-success.sh`](check-main-ci-success.sh) for test skip
4. **reuse-bundle** — when resolver hits: download/validate/re-upload current-run `bindings-*`
5. **build-linux / build-host / stage-cached** — only when bundle not reused; compile/stage cache misses
6. **test** — **skipped** only when `main_validated` (exact SHA passed main); fingerprint reuse across SHAs still runs tests
7. **publish** — validate current-run manifests, `napi artifacts`, `npm publish`, GitHub Release
8. **sync-main-package-lock** — checkout `main`, [`post-release-sync-main-package-lock.sh`](post-release-sync-main-package-lock.sh), open PR `chore/post-release-package-lock-X.Y.Z` (merge promptly — see RELEASE.md)

Release prep on git uses `SKIP_LOCK_REFRESH=1` with [`bump-workspace-versions.sh`](bump-workspace-versions.sh); post-publish sync runs full bump + [`refresh-package-lock-optional-bindings.sh`](refresh-package-lock-optional-bindings.sh).

---

## CI Docker images

| Image                                                     | Dockerfile                                                         | Used for                                                                  |
| --------------------------------------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------- |
| `ghcr.io/<owner>/node-webrtc-rust/ci-build:latest`        | [`docker/ci/Dockerfile`](../../docker/ci/Dockerfile)               | glibc native builds (Linux gnu x64), release builds                       |
| `ghcr.io/<owner>/node-webrtc-rust/ci-build-alpine:latest` | [`docker/ci/Dockerfile.alpine`](../../docker/ci/Dockerfile.alpine) | **musl** native builds (`x86_64-unknown-linux-musl`)                      |
| `ghcr.io/<owner>/node-webrtc-rust/ci-build:test`          | [`docker/ci/Dockerfile.test`](../../docker/ci/Dockerfile.test)     | `ci-build` + coturn + Sherpa models (integration tests on ephemeral pods) |

**ci-build:** Ubuntu 24.04, Node 20, Rust 1.99.0 (pinned in `rust-toolchain.toml`) + Linux cross targets, Zig (napi `--zig` for non-gnu targets).  
**ci-build-alpine:** Node 24 Alpine, Rust + musl toolchain via [`install-alpine-native-toolchain.sh`](install-alpine-native-toolchain.sh).

**ci-test** (tag `ci-build:test` / `ci-build:test-<sha>`; the workflow token cannot create a new GHCR package, so it shares the existing `ci-build` package): `ci-build` plus coturn (`turnserver` runs inside the job container, no sidecar) and the five Sherpa model dirs the integration suite uses (en Kroko STT, en amy-low TTS, Whisper tiny LID, de thorsten-medium and es glados-medium TTS) under `/opt/sherpa-models`, exported as `NWR_CI_SHERPA_MODELS`. [`ensure-sherpa-models.sh`](ensure-sherpa-models.sh) symlinks them into the example's `.models/` when the variable is set; unset (plain `ci-build`, local dev) it behaves as before. It is a separate Dockerfile on purpose: the `ci-build` content digest (`ci-image-content-digest.sh glibc`) is a native build fingerprint input, so model changes must not touch `docker/ci/Dockerfile`. PRs that touch the image inputs run a build-only check in the CI Docker image workflow (nothing is pushed; the base is the published `ci-build:latest`). Guard: [`ci-test-image-models.test.sh`](ci-test-image-models.test.sh).

Rebuild when either Dockerfile (or the Alpine install script) changes:

```bash
# Preferred: push to ci branch, or merge docker/ci changes to main (path-filtered workflow)
git push origin ci

# Or: Actions → CI Docker image → Run workflow (workflow_dispatch)
```

If release prep bumps versions before platform packages are on npm, run:

```bash
SKIP_LOCK_REFRESH=1 bash scripts/ci/bump-workspace-versions.sh <version>
bash scripts/ci/sync-lock-workspace-versions.sh   # keeps lock workspace versions in sync
```

After publish: `bash scripts/ci/refresh-package-lock-optional-bindings.sh` (or merge the post-release PR).

**Before the first musl CI run after adding `Dockerfile.alpine`:** publish `ci-build-alpine:latest` (merge to `main` or push `ci`, then wait for **CI Docker image** workflow). Musl jobs disable npm cache (BusyBox `tar` lacks GNU `-P`).

**Native build env:** `audiopus_sys` needs static Opus + CMake policy shim. Set `OPUS_STATIC=1` and `CMAKE_POLICY_VERSION_MINIMUM=3.5` on reusable build workflows and in [`ci-build-native-*`](../../.github/actions/) build steps (caller workflow `env` does not propagate into `workflow_call` jobs).

---

## Scripts reference

| Script                                                                                                                                  | Used by                                                 | What it runs                                                                                                                                                     |
| --------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`bump-workspace-versions.sh`](bump-workspace-versions.sh)                                                                              | Release prep (`SKIP_LOCK_REFRESH=1`), post-release sync | Bump workspace `package.json` / pins; optional lock refresh + validate                                                                                           |
| [`refresh-package-lock-optional-bindings.sh`](refresh-package-lock-optional-bindings.sh)                                                | After publish, via bump or post-release sync            | Prune stub optional bindings + `npm install`                                                                                                                     |
| [`validate-package-lock-optional-bindings.sh`](validate-package-lock-optional-bindings.sh)                                              | `validate-package-lock` job, before every `npm ci`      | Fail fast on stub optional `@node-webrtc-rust/bindings-*` lock entries (`Invalid Version:`)                                                                      |
| [`post-release-sync-main-package-lock.sh`](post-release-sync-main-package-lock.sh)                                                      | Release `sync-main-package-lock` job after publish      | Bump + refresh lock from npm; workflow opens PR to `main`                                                                                                        |
| [`run-pr-quality.sh`](run-pr-quality.sh)                                                                                                | PR quality job                                          | lock validate, `npm ci`, **`fix-rollup-native.sh`**, typecheck, lint, **`run-helpers-unit-tests.sh`**, Sherpa typecheck + **roundtrip Vitest**                   |
| [`run-helpers-unit-tests.sh`](run-helpers-unit-tests.sh)                                                                                | quality job, `npm run test:helpers`                     | vitest `@node-webrtc-rust/helpers` unit (excludes `*.integration.test.ts`) + multi-client example (no `.node`)                                                   |
| [`run-pre-push-gates.sh`](run-pre-push-gates.sh)                                                                                        | `npm run ci:pre-push`                                   | eslint + build-ts + helpers vitest when scoped; Sherpa **typecheck + Vitest + E2E** when example/speech changes                                                  |
| [`install-pre-push-hook.sh`](install-pre-push-hook.sh)                                                                                  | one-time per clone                                      | installs `.git/hooks/pre-push` → `npm run ci:pre-push`                                                                                                           |
| [`run-if-helpers-changed.sh`](run-if-helpers-changed.sh)                                                                                | alias                                                   | → `run-pre-push-gates.sh`                                                                                                                                        |
| [`plan-native-builds.sh`](plan-native-builds.sh)                                                                                        | main + release plan job                                 | Per-target cache hash check → dynamic build matrices                                                                                                             |
| [`check-main-ci-success.sh`](check-main-ci-success.sh)                                                                                  | release plan job                                        | Skip release test when main validated same SHA                                                                                                                   |
| [`list-release-targets.sh`](list-release-targets.sh)                                                                                    | plan / stage / fingerprint contract                     | Canonical six release triples (source of truth)                                                                                                                  |
| [`native-build-fingerprint.sh`](native-build-fingerprint.sh) / [`native_build_contract.py`](native_build_contract.py)                   | main/release/PR native jobs                             | Target-specific digests, aggregate, bundle assemble/validate                                                                                                     |
| [`native-artifact-manifest.sh`](native-artifact-manifest.sh) / [`write-native-artifact-manifest.sh`](write-native-artifact-manifest.sh) | build/stage                                             | Produce/validate per-target provenance manifests                                                                                                                 |
| [`native-artifact-bundle.sh`](native-artifact-bundle.sh)                                                                                | main assemble / release validate                        | Six-target `native-main-bundle`                                                                                                                                  |
| [`resolve-native-main-bundle.sh`](resolve-native-main-bundle.sh)                                                                        | release plan                                            | Trust-checked main-bundle resolver (REST; no `gh`)                                                                                                               |
| [`collect-native-tool-identity.sh`](collect-native-tool-identity.sh)                                                                    | fingerprint/manifest jobs                               | Declared + resolved tool identity exports                                                                                                                        |
| [`check-release-targets.sh`](check-release-targets.sh)                                                                                  | contract tests / CI verify                              | Six-target ↔ npm platform package / optionalDependency completeness                                                                                              |
| [`native-cache-epoch`](native-cache-epoch)                                                                                              | fingerprint inputs                                      | Manual epoch — bump to force-rebuild all native digests                                                                                                          |
| [`verify-release-publish-ts.sh`](verify-release-publish-ts.sh)                                                                          | Local release publish TS parity                         | `npm ci --ignore-scripts`, version bump, `build-ts-workspace.sh`                                                                                                 |
| [`build-ts-workspace.sh`](build-ts-workspace.sh)                                                                                        | PR build-ts + integration fallback                      | sdk core → signaling → full sdk                                                                                                                                  |
| [`run-pr-integration.sh`](run-pr-integration.sh)                                                                                        | PR test job                                             | [`npm-ci-workspace.sh`](npm-ci-workspace.sh), cargo test (incl. speech), optional build:ts, npm test, [`run-sherpa-example-ci.sh e2e`](run-sherpa-example-ci.sh) |
| [`run-sherpa-example-ci.sh`](run-sherpa-example-ci.sh)                                                                                  | quality (`typecheck`, `vitest`) + test (`e2e`)          | Sherpa `tsc`; **all** `test:roundtrip-counting` Vitest; **all** `start:roundtrip-*` E2E after model download                                                     |
| [`run-sherpa-roundtrip-e2e.sh`](run-sherpa-roundtrip-e2e.sh)                                                                            | via `run-sherpa-example-ci.sh e2e`                      | CI pass streams `[speech]` events; `[voice-debug]` off unless re-run on failure                                                                                  |
| [`ci-step.sh`](ci-step.sh)                                                                                                              | integration + Sherpa E2E                                | `[ci-step] START/OK/FAIL` banners; optional `--timeout` via [`run-with-timeout.sh`](run-with-timeout.sh)                                                         |
| [`run-with-timeout.sh`](run-with-timeout.sh)                                                                                            | via `ci-step.sh`                                        | GNU `timeout` / `gtimeout` wall-clock cap per step                                                                                                               |
| [`run-pr-test-job-docker.sh`](run-pr-test-job-docker.sh)                                                                                | `npm run ci:verify:pr-test:docker`                      | **Optional** `ci-test` container (starts coturn itself) → `run-pr-integration.sh`                                                                                |
| [`run-pr-tests-full.sh`](run-pr-tests-full.sh)                                                                                          | `npm run ci:verify:pr-full`                             | quality + integration (host)                                                                                                                                     |
| [`run-pr-integration.sh`](run-pr-integration.sh)                                                                                        | main + release test                                     | integration only (after quality job)                                                                                                                             |
| [`verify-checks.sh`](verify-checks.sh)                                                                                                  | `npm run ci:verify:checks*`                             | Local mirror of quality + integration                                                                                                                            |
| [`ensure-workspace-bindings.sh`](ensure-workspace-bindings.sh)                                                                          | via [`npm-ci-workspace.sh`](npm-ci-workspace.sh)        | Remove nested registry `bindings` copies so `npm test` loads workspace `.node`                                                                                   |
| [`verify-linux.sh`](verify-linux.sh)                                                                                                    | `npm run ci:verify:linux`                               | Local release cross-builds in Docker                                                                                                                             |

---

## Local validation

Run these **before pushing CI changes** (see [`.cursor/rules/ci-local-validation.mdc`](../../.cursor/rules/ci-local-validation.mdc)):

```bash
npm run build:native                           # host .node for npm test
bash scripts/ci/run-pr-quality.sh              # PR quality job
bash scripts/ci/verify-release-publish-ts.sh   # release publish TS path
npm run ci:verify:release-ts                     # same as verify-release-publish-ts.sh
bash scripts/ci/build-ts-workspace.sh          # PR build-ts job (from clean dist/)
npm run ci:verify:pr-full                        # quality + integration (host)
npm run ci:verify:checks                         # full PR check suite (host)
npm run ci:verify                                # alias for ci:verify:checks
npm run ci:verify:linux                          # optional: release Linux cross-builds in Docker
```

**Optional Docker parity** (remote ci-build container only):

```bash
npm run ci:verify:checks:docker
npm run ci:verify:release-ts:docker
npm run ci:verify:pr-test:docker
npm run ci:docker:build                        # build ci-build image locally
```

After changing `docker/ci/Dockerfile`, rebuild and push to the `ci` branch before expecting Linux CI jobs to pick up toolchain changes.

---

## Native fingerprint, provenance, and main→release reuse

### Digests

| Digest           | Script                                                  | Includes                                                                                                                                                                                                                                                                                                                                                                                                                  | Excludes                                                                                                            |
| ---------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| **Native input** | `native-build-fingerprint.sh --target T --profile P`    | `Cargo.lock`, Cargo versions, complete local deps via `cargo metadata`, sources/`build.rs`, features (`otel` on release), canonical [`build-native-addon.sh`](build-native-addon.sh) recipe, target-specific toolchain files (musl-only for musl), declared tool contract, [`native-cache-epoch`](native-cache-epoch). Text inputs are hashed with CRLF normalized to LF so Windows producers match Linux/macOS assemble. | npm package **version**, generated `index.js` / `index.d.ts`, cache/manifest/bundle/resolver/planner implementation |
| **Distribution** | `native-build-fingerprint.sh --distribution --target T` | N-API surface + npm manifests (**including** version)                                                                                                                                                                                                                                                                                                                                                                     | Rust sources                                                                                                        |
| **Aggregate**    | `native-artifact-bundle.sh aggregate`                   | Sorted `target=input_digest` over the six release triples                                                                                                                                                                                                                                                                                                                                                                 | —                                                                                                                   |

CI uses `NATIVE_TOOL_MODE=declared` so plan/build share per-target tool contracts (`ci-build` / Alpine / `ubuntu-24.04-arm` / `macos-latest` / `windows-latest`, Node major, `stable` channel, Zig 0.14.1, lockfile `@napi-rs/cli`). **Image identity** is a content digest (or registry `@sha256` / immutable `:SHA` tag) — never bare `:latest`. Host-resolved `rustc -Vv` etc. go into provenance as `tool_identity_resolved`, not the cache key. Local tests default to `unresolved`.

**Force-rebuild all native digests:** bump [`native-cache-epoch`](native-cache-epoch) (committed) or set env `NATIVE_CACHE_EPOCH` for a one-off run. Any compile-semantic change outside the canonical recipe (runner image/toolchain contract, linker environment, target wiring) must update its declared contract or bump this epoch; orchestration-only changes must not.

**Alpine / musl Actions cache:** BusyBox `/bin/tar` rejects GNU options (`--posix`, `-P`) used by `actions/cache` and Swatinem. Install/link GNU tar via [`install-alpine-native-toolchain.sh`](install-alpine-native-toolchain.sh) and runtime [`ensure-gnu-tar-alpine.sh`](ensure-gnu-tar-alpine.sh) before musl restore/save.

**Windows path stability:** contract relative paths always use forward slashes (`Path.as_posix()`), so Linux planners and Windows producers share one `native-v3-*` key.

**Bare self-hosted plan/assemble (no job `container:`):** fingerprinting and bundle validate/assemble call `cargo metadata` via `docker run` against `CI_IMAGE` when `CARGO_VIA_CI_IMAGE=1` ([`native_build_contract.py`](native_build_contract.py)). Do **not** install `dtolnay/rust-toolchain` on the runner host — host `rustup` can race with other jobs (e.g. `bin/rust-gdb` conflicts). Jobs that already use `container: ci-build` keep host `cargo` on `PATH` and must **not** set `CARGO_VIA_CI_IMAGE` (docker-in-docker). Docker metadata may leave root-owned `target/`; the existing alpine `chown` prepare step on the next bare job resets workspace ownership.

### Per-target Actions cache (rebuild accelerator only)

Key shape: `native-v3-{profile}-{target}-{input_digest}`.

- Exact key match only (curl + equality filter — never trust API `total_count` alone).
- Linux **and** host (macOS/Windows) jobs **save** the `.node` + provenance manifest after every compile miss.
- Cache restore/validation failures fall back to compile. Planner misses → schedule build.

### Provenance manifests

Produced/validated on every build and stage path (`write-native-artifact-manifest.sh`). Uploaded beside `.node` inside `bindings-<triple>/` artifacts (`*.node` + `manifest.json`) so `napi artifacts` still works. Producer-workspace paths are informational after upload: assembly and bundle validation explicitly bind each manifest checksum to the canonical `.node` beside it.

### Main bundle (authoritative reusable artifact)

After a successful main Test job, [`build-main.yml`](../../.github/workflows/build-main.yml) assembles all six `bindings-*` artifacts into **`native-main-bundle`** (retention **90 days**) keyed by `meta.json` → `aggregate_digest`. Assembly validates the copied bundle bytes, manifest identity, and metadata/checksum agreement before upload. Release deliverables are **not** stored only in Actions Cache.

### Release reuse (trust boundaries)

[`resolve-native-main-bundle.sh`](resolve-native-main-bundle.sh) (GitHub REST via curl/urllib — **no `gh`**):

1. List successful `build-main.yml` runs on **`main`** only.
2. Prefer **exact `GITHUB_SHA`**, then older runs whose `aggregate_digest` matches the current contract.
3. Require artifact name `native-main-bundle`, not expired.
4. Download by artifact id + token; validate every target manifest/checksum against the **current** workspace contract.
5. Reject wrong workflow/branch, malformed JSON, missing/expired artifacts, fingerprint mismatch, validation failure → `bundle_reused=false` and rebuild.

Release behavior:

| Situation                                          | Native builds                                                                   | Integration tests                                                 | Publish                              |
| -------------------------------------------------- | ------------------------------------------------------------------------------- | ----------------------------------------------------------------- | ------------------------------------ |
| Valid main bundle (exact SHA or fingerprint match) | Skip matrix; stage bundle into current-run `bindings-*`                         | Skip **only** if exact SHA already passed main (`main_validated`) | Validated current-run artifacts only |
| Missing/expired/invalid bundle                     | Per-target cache plan + rebuild misses                                          | Run (unless exact SHA main_validated with warm caches)            | Same                                 |
| Rebuild/cache failure                              | Fail the job (retryable) — never permanent publish block from “no bundle” alone | —                                                                 | —                                    |

### Six release targets

[`list-release-targets.sh`](list-release-targets.sh) is canonical. [`check-release-targets.sh`](check-release-targets.sh) enforces npm dirs + `optionalDependencies`. Loader fallbacks in `index.js` beyond the six are allowed.

### Observability (`GITHUB_STEP_SUMMARY`)

Plan / main / release / smoke append a short summary via [`write-native-ci-summary.sh`](write-native-ci-summary.sh): aggregate fingerprint, cache hits, rebuilt targets, producer SHA/run (when reused), preference, and fallback reason. No tokens or secrets.

### Native cache smoke (manual, non-publishing)

[`native-cache-smoke.yml`](../../.github/workflows/native-cache-smoke.yml) is **`workflow_dispatch` only** (no push/PR). It never runs `npm publish` or creates a GitHub Release. It uploads **`native-smoke-bundle`** (14d) — **not** `native-main-bundle` (release trust boundary).

**Two-dispatch procedure:**

1. **Actions → Native cache smoke → Run workflow → `phase=first-build`**
   Plan → compile cache misses → stage hits → assemble/validate `native-smoke-bundle`. Summary lists `rebuilt_targets` / `all_cached`.
2. **Same ref → `phase=reuse-check`**
   Fails unless `all_cached=true` (zero native compile). Still runs release-style `resolve-native-main-bundle` (read-only against successful `main` runs) and records preference/fallback in the summary.

### Local contract tests

```bash
bash scripts/ci/native-build-fingerprint.test.sh
bash scripts/ci/native-artifact-manifest.test.sh
bash scripts/ci/native-artifact-bundle.test.sh
bash scripts/ci/resolve-native-main-bundle.test.sh
bash scripts/ci/check-release-targets.test.sh
bash scripts/ci/native-binding-cache-key.test.sh
bash scripts/ci/ts-dist-cache-key.test.sh
bash scripts/ci/ci-cache-layers-workflow.test.sh
bash scripts/ci/cargo-via-ci-image.test.sh
bash scripts/ci/compute-pr-job-gates.test.sh
bash scripts/ci/pr-workflow-gates.test.sh
bash scripts/ci/ci-test-image-models.test.sh
```

Wired at the top of [`verify-checks.sh`](verify-checks.sh).

## Caching architecture (layers, retention, trust)

| Layer                            | Key / identity                                                              | Paths                                                         | Retention / eviction                              | Trust                                                                                                                                                                                                                |
| -------------------------------- | --------------------------------------------------------------------------- | ------------------------------------------------------------- | ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Native `.node` (accelerator)** | `native-v3-{profile}-{target}-{input_digest}`                               | `packages/bindings/*.node` + `native-manifests/<target>.json` | GHA cache (LRU / org limits)                      | Exact key + surface verify + provenance; miss → compile                                                                                                                                                              |
| **`native-main-bundle`**         | Aggregate digest in `meta.json`                                             | workflow artifact                                             | **90 days**                                       | Only from successful `build-main.yml` on `main`; release validates vs current contract                                                                                                                               |
| **`native-smoke-bundle`**        | Same assemble/validate                                                      | smoke artifact                                                | **14 days**                                       | Manual smoke only — **not** consumed by release                                                                                                                                                                      |
| **Cargo `target/` (Linux)**      | `cargo-tgt-v1-{profile}-{target}-{cache_prefix}-{hash(lock+docker inputs)}` | `target/`                                                     | GHA cache; **exact key only** (no `restore-keys`) | Saves after miss; workspace `rm -rf target` after save; never cross-target prefix restore (Sherpa/stale safeguard)                                                                                                   |
| **Cargo registry**               | Swatinem `cargo-reg-v1-…` (Linux) / `cargo-v1-…` (host)                     | registry/git (Swatinem)                                       | GHA via Swatinem                                  | Linux: registry only (`cache-targets: false`). Host: registry **+** that target’s `target/` in **one** Swatinem entry (no overlapping `actions/cache` for `target/`)                                                 |
| **TS `dist/`**                   | `ts-dist-v2-node{major}-{digest}`                                           | sdk/signaling/helpers `dist/`                                 | GHA cache                                         | Digest = manifests + lock + build scripts + sources + tsconfigs + Node major. No pr/release namespace (identical outputs). [`ensure-ts-dist.sh`](ensure-ts-dist.sh) requires stamp + required `index.js` or rebuilds |
| **Docker BuildKit**              | scopes `ci-build-glibc` / `ci-build-alpine`                                 | Buildx GHA cache                                              | GHA BuildKit cache                                | Distinct scopes so glibc/Alpine layers never collide. Images tagged `:latest` **and** `:{github.sha}`; fingerprints use content/@sha256                                                                              |

### Intentional non-caches

| Not cached                                            | Why                                                                                               |
| ----------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| **`node_modules` / npm**                              | Measured multi-GB polluted `~/.npm` / workspace transfers; `npm ci` is the contract               |
| **Cross-job Cargo test `target/` artifact (~500 MB)** | Measured net-negative handoff between compile and test jobs; test profile differs from NAPI addon |
| **Mutable `ci-build:latest` as fingerprint identity** | Convenience pull tag only; contract is content digest / digest / SHA tag                          |

### CI Docker image triggers

[`ci-image.yml`](../../.github/workflows/ci-image.yml) rebuilds when `docker/ci/**`, `install-alpine-native-toolchain.sh`, or **`build-sherpa-onnx-musl-libs.sh`** (COPY’d into Alpine) change. Record content digests in the job summary.

`actions/setup-node` **`cache: npm` is disabled**. PR native profile: **debug**. Main/release/smoke: **release** (`v3-release-otel` Cargo prefix; binding keys `native-v3-*`).

---

## Composite actions

| Action                                                                 | Purpose                                                                                         |
| ---------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| [`native-binding-cache`](../../.github/actions/native-binding-cache)   | Per-target `native-v3` restore/validate; outputs `cache-key` for save                           |
| [`ci-build-native-linux`](../../.github/actions/ci-build-native-linux) | Restore/build/verify/manifest/**save**/upload Linux `.node`; exact Cargo `target/` restore+save |
| [`ci-build-native-host`](../../.github/actions/ci-build-native-host)   | Same semantics for macOS/Windows (Swatinem registry+target)                                     |
| [`plan-native-builds`](../../.github/actions/plan-native-builds)       | Exact per-target cache probe + matrices + summary                                               |
| [`ci-cache-ts-dist`](../../.github/actions/ci-cache-ts-dist)           | sdk/signaling/helpers `dist/` cache (`ts-dist-v2-node20-*`)                                     |
