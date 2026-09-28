# Prompt: `@node-webrtc-rust/voice-catalog` (new branch)

Copy everything below the line into a **new** agent session. Do **not** continue the cloud-vendor-streaming feature branch for this work — open a new `feature/` branch from latest `origin/main` (worktree flow).

---

## Goal

Add a **standalone published npm package** `@node-webrtc-rust/voice-catalog` (no native bindings, no NAPI, no SDK dependency) so **one JSON list** of vendor ids, default models/voices, documented model allowlists, official doc URLs, and large catalogs (e.g. Azure TTS ShortNames) is consumed by:

1. **Rust** vendor crates — `include_str!` the same JSON (or generated once from it). Delete duplicated `DOCUMENTED_*` string arrays / `voices.rs` copies.
2. **TypeScript** in this repo — `examples/shared/voice-vendor-docs.ts` (and presets if needed) import the package instead of redefining ids.
3. **Later consumers** (VoiceThere platform, CLI) — `npm i @node-webrtc-rust/voice-catalog` after publish. Do **not** add a `file:` dependency that only works in a sibling worktree. Do **not** make platform depend on `@node-webrtc-rust/sdk` (native).

## Constraints

- Official vendor docs only. Do not invent models, voices, or endpoints. Snapshot date the Azure (and any other) full list and cite the Microsoft language-support / GET voices/list URLs.
- Public package: **no** VoiceThere `AGENT_*` secret names, no platform billing copy, no runner env maps.
- New workspaces: `npm install` and commit `package-lock.json` ([package-lock workspace rule](../../.cursor/rules/node-webrtc-rust-package-lock-workspace.mdc) if present).
- Wire `scripts/release-publish.sh` / `build-ts-workspace.sh` / `ci:verify:release-ts` so the new package publishes with `@node-webrtc-rust/*`.
- Branch: `feature/voice-catalog-package` (or `chore/voice-catalog-package`) from **latest `origin/main`**. Never `cursor/` or `feat/`.
- Worktree: `.worktrees/voice-catalog-package/node-webrtc-rust`. TokenSave cwd = feature folder only.
- Do **not** pin VoiceThere production. Do **not** change platform/runner/cli in this PR unless you only add a comment “pin after nwr npm”.
- `node-webrtc-rust` `main` is protected — PR to `main` (`owner: akirilyuk`).

## Suggested catalog shape

`packages/voice-catalog/catalog/`:

- `vendors.json` — array of `{ id, label, stt, tts, defaultSttModel?, defaultTtsModel?, defaultTtsVoice?, sttModels[], ttsModels[], ttsVoices[] | ttsVoiceListFile, home, sttDocs?, ttsDocs?, modelsDocs? }`. Include `mock` and `local-sherpa`. Exclude VoiceThere-only notes.
- `azure-tts-voices.json` — the 761 ShortNames already snapshotted in `crates/vendor-azure/azure-tts-voices.txt` (move that file here; rust `include_str!`).
- Optional per-vendor JSON if lists stay large (AWS Polly sample voices, Groq models, Google STT models, Deepgram listen models, Cartesia sonic ids, AssemblyAI speech_model, OpenAI STT/TTS models).

`src/index.ts` exports typed constants (`STT_PROVIDER_IDS` without forcing platform’s subset — export `customerSttProviderIds` vs `allVendorIds` if `mock` must stay out of dashboards).

Rust: parse JSON once (`once_cell` / `LazyLock`) or `include_str` + serde. Keep **transport** logic in each `matrix.rs` (that is not a name list).

## Verify

```bash
cd node-webrtc-rust
mkdir -p .test-logs
# package tests + typecheck
npm run typecheck --workspace=@node-webrtc-rust/voice-catalog
npm test --workspace=@node-webrtc-rust/voice-catalog
# rust still validates lists
cargo test -p node-webrtc-rust-vendor-azure -p node-webrtc-rust-vendor-groq -p node-webrtc-rust-vendor-openai --lib
```

Redirect full output to `.test-logs/`. Format only edited paths.

## Done when

- Single JSON (or small set of JSON files) is the only name/voice/model allowlist.
- Package is in the workspace lockfile and release publish list.
- PR open to `main` with crate READMEs still pointing at streaming **decisions** (do not delete those).
- Report package name, version bump plan (same as current nwr semver), and that platform/cli pin is a **follow-up after npm publish**.
