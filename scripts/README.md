# Development scripts

Helper scripts used by examples and local dev (not CI-only). Run from **`node-webrtc-rust`** repo root unless noted.

## `free-port.sh`

Frees a TCP port before an example server binds (avoids `EADDRINUSE` after a crashed or background `npm run start`).

| Item | Detail |
| ---- | ------ |
| **Default usage** | `PORT=3004 bash scripts/free-port.sh [label]` |
| **With port arg** | `bash scripts/free-port.sh 3004 my-label` |
| **Chained start** | `bash scripts/free-port.sh 3004 label -- tsx src/index.ts` |
| **Skip** | `VOICE_SKIP_FREE_PORT=1` |
| **Implementation** | `lsof -ti :PORT -sTCP:LISTEN` (Unix), `netstat` + kill (Windows) |

**npm `prestart`:** [`voice-agent-local-sherpa-multi-client`](../examples/voice-agent-local-sherpa-multi-client/package.json) runs this automatically before `start`, `start:cap-2`, and `start:debug` (port **3004** or `PORT`).

**In-process backup:** several examples also call [`examples/shared/free-port.ts`](../examples/shared/free-port.ts) at startup. Shell + TS both respect `VOICE_SKIP_FREE_PORT=1`.

```bash
# Multi-client example (prestart runs automatically):
npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client

# Manual, from repo root:
PORT=3004 bash scripts/free-port.sh voice-agent-local-sherpa-multi-client
```

## `export-sherpa-local-models.sh`

Exports `SHERPA_STT_MODEL_PATH`, `SHERPA_TTS_MODEL_PATH`, and `SHERPA_STT_LANGUAGE` for local Sherpa examples. Used as a prefix on `npm run start*` in `voice-agent-local-sherpa` and `voice-agent-local-sherpa-multi-client`.

```bash
source scripts/export-sherpa-local-models.sh
bash scripts/export-sherpa-local-models.sh -- tsx path/to/server.ts
```

See [`examples/voice-agent-local-sherpa/README.md`](../examples/voice-agent-local-sherpa/README.md).

## Release and package-lock scripts

Publishing and version bumps: **[`RELEASE.md`](RELEASE.md)** (full guide).

| Script | Purpose |
| ------ | ------- |
| [`ci/bump-workspace-versions.sh`](ci/bump-workspace-versions.sh) | Bump all `@node-webrtc-rust/*` versions in git |
| [`ci/post-release-sync-main-package-lock.sh`](ci/post-release-sync-main-package-lock.sh) | After npm publish — align `main` lockfile from registry |
| [`ci/refresh-package-lock-optional-bindings.sh`](ci/refresh-package-lock-optional-bindings.sh) | Regenerate optional platform binding lock entries |
| [`ci/validate-package-lock-optional-bindings.sh`](ci/validate-package-lock-optional-bindings.sh) | Detect stub lock entries (`npm run ci:validate:package-lock`) |

Release prep: `SKIP_LOCK_REFRESH=1 bash scripts/ci/bump-workspace-versions.sh X.Y.Z`. After tag publish, merge the automated **`chore/post-release-package-lock-X.Y.Z`** PR (see RELEASE.md).

## CI scripts

See [`scripts/ci/README.md`](ci/README.md).
