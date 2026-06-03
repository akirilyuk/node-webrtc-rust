# Changelog

All notable changes to this project are documented here.

Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased]

---

## [0.5.2] — 2026-06-03

**Compare:** [`release/0.5.1…release/0.5.2`](https://github.com/akirilyuk/node-webrtc-rust/compare/release/0.5.1...release/0.5.2)

Barge-in STT pre-roll keeps the first spoken syllable during agent TTS, plus release lockfile automation and CI guards.

### Highlights

- **Barge-in first syllable** — larger default `speechPadMs` (500 ms) and STT pre-roll flush timing so the opening word is not dropped when interrupting agent playback.
- **Release lockfile sync** — post-publish workflow opens a PR to refresh `package-lock.json` optional binding entries from npm; release prep uses `SKIP_LOCK_REFRESH=1` on version bump.
- **`validate-package-lock`** — always-on CI job and pre-`npm ci` check to catch stub optional binding entries before they cause opaque `Invalid Version:` failures.

### Fixed

- Barge-in TTS drain integration test stabilized for CI (deterministic drain wait, harness timing).
- Sherpa barge-in roundtrip: Phase 3 waits for `user_speech_final` after semantic interrupt.

### Changed

- `validate-package-lock-optional-bindings.sh` + always-on **`validate-package-lock`** CI job on PR / `main` / release (no path filter); `npm ci` paths and `bump-workspace-versions.sh` run the same check.
- Release workflow **`sync-main-package-lock`** job after publish — checks out `main`, syncs lockfile from npm, opens PR `chore/post-release-package-lock-X.Y.Z`. Documented in [`scripts/RELEASE.md`](scripts/RELEASE.md#package-lockjson-after-release).
- Release prep branches use `release-prep/X.Y.Z`; publish tags remain `release/X.Y.Z`.

### Docs

- [`scripts/RELEASE.md`](scripts/RELEASE.md), [`scripts/ci/README.md`](scripts/ci/README.md) — package-lock after release flow.

---

## [0.5.1] — 2026-06-03

**Compare:** [`release/0.5.0…release/0.5.1`](https://github.com/akirilyuk/node-webrtc-rust/compare/release/0.5.0...release/0.5.1)

VAD pause grace defaults, more reliable multi-client localhost WebRTC, Sherpa roundtrip timing fixes, and dev scripts for ports and Sherpa model export.

### Highlights

- **VAD pause grace** — default `minSilenceDurationMs` 1300 and `sttGateHoldMs` 1000 so short intra-utterance pauses are less likely to end the turn early.
- **Multi-client WebRTC** — send answer immediately (trickle ICE); replace stale sessions on re-join; auto-reconnect on `connectionState=failed`; default `WEBRTC_NAT_1TO1_IPS=127.0.0.1` for localhost; server timestamps on `speech_event` and logs.
- **Sherpa barge-in E2E** — inter-phase STT drain (~3.15 s) between Phase 2 and 3 for a fresh `vad_triggered`; counting roundtrip wall clock scaled for 1300 ms VAD silence.

### Added

- `scripts/free-port.sh` and `scripts/export-sherpa-local-models.sh`; multi-client `prestart` frees port 3004.
- Pause-mic-when-background UI under Connect; `?pauseMicBackground=1` for non-speaking tabs.

### Changed

- `refresh-package-lock-optional-bindings.sh` — regenerate lockfile optional bindings; prevent invalid stub entries after version bumps.
- Sherpa CI roundtrip harness: barge-in allows STT session open across phases; drain between barge phases.

### Docs

- [`packages/sdk/VOICE-VAD-AND-BARGE-IN.md`](packages/sdk/VOICE-VAD-AND-BARGE-IN.md), [`examples/voice-agent-local-sherpa/ROUNDTRIP.md`](examples/voice-agent-local-sherpa/ROUNDTRIP.md), [`scripts/README.md`](scripts/README.md).

---

## [0.5.0] — 2026-06-02

**Compare:** [`release/0.4.1…release/0.5.0`](https://github.com/akirilyuk/node-webrtc-rust/compare/release/0.4.1...release/0.5.0)

Reworked VAD → STT → barge-in pipeline: recognition opens on speech detection instead of an always-on feed during agent TTS, with explicit lifecycle events and utterance-close timeouts.

### Highlights

- **Reworked VAD + bargeIn + STT forwarding** — STT stream opens on each VAD `SpeechStart` (`vad_triggered` → `user_stt_start` → `stt_stream_start`) with pre-roll flush preserved; PCM is no longer fed continuously to STT during agent playback for semantic barge.
- **Tighter barge window** — `barge_in` only while `agent_speaking == true` (removed 2s post-playback grace); `requireSttPartial: true` still gates interrupt on real words, `false` barge is immediate on `SpeechStart`.
- **Observable STT lifecycle** — new events (`vad_triggered`, `stt_stream_*`, `user_stt_*`) for debugging and browser parity; Sherpa roundtrip E2E asserts the full sequence across barge-in, counting, two-phrase, utterance-timing, and barge-recovery scripts.
- **Utterance close paths** — `sttListenTimeoutMs` (C1: VAD without partial → `user_stt_not_found`, no final) and `utteranceFinalizeTimeoutMs` (C2: partial stall → forced `user_speech_final` from last partial).

### Added

- Speech lifecycle events: `vad_triggered`, `stt_stream_start`, `stt_stream_end`, `user_stt_start`, `user_stt_end`, `user_stt_not_found`.
- VAD config: `sttListenTimeoutMs` (default 4000) and `utteranceFinalizeTimeoutMs` (default 1500).
- Shared Sherpa roundtrip STT lifecycle evaluators (`roundtrip-stt-lifecycle-helpers.ts`) and Rust barge-in unit tests.

### Changed

- **STT during agent TTS** — opens on VAD `SpeechStart`, not continuous pre-VAD PCM feed during semantic barge.
- **Barge window** — `barge_in` only while `agent_speaking == true`.
- **Utterance close** — partials without vendor final get `user_speech_final` via gate-hold finalize fallback or C2 timeout (last partial text).

### Docs

- [`packages/sdk/VOICE-VAD-AND-BARGE-IN.md`](packages/sdk/VOICE-VAD-AND-BARGE-IN.md) — STT utterance lifecycle (flows A/B/C1/C2/D), timer interaction, updated fine-tuning timeline.
- [`packages/sdk/VOICE-API.md`](packages/sdk/VOICE-API.md) — lifecycle event order reference.
- [`examples/voice-agent-local-sherpa/ROUNDTRIP.md`](examples/voice-agent-local-sherpa/ROUNDTRIP.md) — STT lifecycle evaluators and updated barge-in E2E criteria.

---

## [0.4.1] — 2026-06-02

**Compare:** [`release/0.4.0…release/0.4.1`](https://github.com/akirilyuk/node-webrtc-rust/compare/release/0.4.0...release/0.4.1)

Snappier Sherpa roundtrip finalize, barge-in Phase 3 `user_speech_final` E2E, echo harness fixes, release CI workspace prep, and git/npm version alignment.

### Highlights

- **Snappier utterance finalize** — `stt_endpoint_tail_ms` capped at 600ms; harness post-TTS silence derived from VAD; `AgentSpeakingEndLatch` ends waits on `agent_speaking_end` instead of long estimate sleeps.
- **Barge-in Phase 3 E2E** — assert `user_speech_final` after semantic `barge_in`; parallel trailing-silence collection so finals are not dropped.
- **Release workflow** — plan job workspace `chown` on self-hosted runners; publish gated on `plan` success.
- **Repo versions** — workspace `package.json` pins synced to published npm (`bump-workspace-versions.sh`); release prep via `release-prep/X.Y.Z` PR before tag.

### Added

- `AgentSpeakingEndLatch`, `waitAgentPlaybackEndRace`, `postTtsSilenceSeconds()` in Sherpa roundtrip harness.
- `evaluateBargeUtteranceFinal`, `phase3EventsTerminal` for barge-in Phase 3 tests.
- [`scripts/ci/bump-workspace-versions.sh`](scripts/ci/bump-workspace-versions.sh) for release prep and catch-up PRs.

### Changed

- Echo / barge-recovery roundtrips: one `speechEvents()` consumer per `VoiceAgent`; `echoVadConfig` disables barge on peer-listen legs; playback baseline before `sendTextToTTS`.
- [`scripts/RELEASE.md`](scripts/RELEASE.md) — versions committed on `release-prep/X.Y.Z` PR before tag (not optional post-publish on `main`).

### Fixed

- Release CI `Permission denied` on plan-job checkout (self-hosted workspace ownership).
- Barge-recovery round 2 waits for Agent2 barge phrase `user_speech_final`.

### Docs

- [`packages/sdk/VOICE-API.md`](packages/sdk/VOICE-API.md), [`ROUNDTRIP.md`](examples/voice-agent-local-sherpa/ROUNDTRIP.md) harness playback timing, voice JSDoc + speech rustdoc.

---

## [0.4.0] — 2026-06-02

**Compare:** [`release/0.3.0…release/0.4.0`](https://github.com/akirilyuk/node-webrtc-rust/compare/release/0.3.0...release/0.4.0)

Semantic STT-gated barge-in, Sherpa roundtrip E2E coverage, and CI hardening for voice dist caches.

### Highlights

- **Semantic barge-in** — agent TTS stops only after STT recognizes real words (`bargeIn.requireSttPartial`, default `true`), not on coughs or tones alone; event order is `user_speech_partial` → `barge_in` → `agent_speaking_end`.
- **Sherpa roundtrip E2E** — seven `start:roundtrip-*` scripts in PR/main integration (counting, two-phrases, utterance-timing, semantic barge-in, counting-echo, barge-recovery).
- **CI** — TS `dist/` stamp/fingerprint rebuild when sources change; `build-ts` job on main; no partial `restore-keys` on dist cache.

### Added

#### Voice pipeline (Rust + `@node-webrtc-rust/sdk/voice`)

- STT-gated barge during agent TTS: partial before `barge_in`, STT nudge on `SpeechEnd`, one barge per agent playback; optional `agentPlaybackGuardMs` (default `0`).
- `SPEECH_EVENT_TYPE` export for typed tests and control-channel bridges.
- Default Sherpa STT model: `en-kroko-2025-08-06`; VAD/env overrides documented.

#### Examples

- Sherpa roundtrip harness: counting, two-phrases, utterance-timing, semantic barge-in, counting-echo, barge-recovery (agent2 barge only; agent1 injects TTS).
- Vitest evaluators (no models) for barge-in event ordering.
- Baseline waiters for `agent_speaking_start` / `barge_in`; `[speech]` logs on, `[voice-debug]` opt-in via `VOICE_DEBUG=1`.

#### `@node-webrtc-rust/helpers`

- Do not block speech events on slow voice handlers (forward before `onSpeechEvent`).

### Changed

- Fix STT finalize on gate-hold expiry; pair `user_speaking_end` with `user_speech_final`.
- Background TTS drain; `agent_speaking_start` / `agent_speaking_end` tied to real PCM playback.
- Sherpa roundtrip phrase tweaks for STT stability (`"Welcome to the demo"`).

### Fixed

- Barge-recovery E2E: disable barge-in on agent1 (injector); STT-gated barge on agent2; arm `waitForBargeIn` before TTS.
- Main CI stale `packages/sdk/dist` after voice API changes — rebuild when stamp mismatches sources.

### CI & tooling

- All Sherpa roundtrips in PR Test; `cargo test -p node-webrtc-rust-speech` in integration.
- Host-local `ci:verify:checks`; `ci-step` timeouts; quiet roundtrip CI wrapper (`VOICE_DEBUG=0`).
- `examples/**` path filter; workspace binding sync for `npm test`.

---

## [0.3.0] — 2026-05-28

**Compare:** [`release/0.2.1…release/0.3.0`](https://github.com/akirilyuk/node-webrtc-rust/compare/release/0.2.1...release/0.3.0)

Voice agent milestone — agentic STT/TTS pipeline, multi-session server helpers, WebRTC P1 parity.

### Highlights

- **Agentic voice stack** — `VoiceAgent` in Rust + `@node-webrtc-rust/sdk/voice` in TypeScript: VAD, barge-in, STT/TTS vendors, speech events up to Node, TTS text down.
- **Multi-session servers** — new `@node-webrtc-rust/helpers` with `SessionPod` and `VoiceAgentSessionHost` (one agent per WebRTC connection, cleanup on hangup).
- **WebRTC P1 parity** — `getStats`, `setConfiguration`, `restartIce`, transceivers, `replaceTrack`, `removeTrack`, `RemoteAudioTrack.readSample`, data-channel backpressure.
- **Local + cloud speech vendors** — OpenAI, Deepgram, ElevenLabs, Google, Cartesia, AssemblyAI, mock, and on-device **Sherpa-ONNX** STT/TTS.
- **CI hardening** — native binding surface verification, precise cache keys, Linux **arm64** host-matrix builds without Zig.

### Added

#### `@node-webrtc-rust/sdk/voice`

- `VoiceAgent` class — attach inbound/outbound audio tracks, `start()` / `stop()`, `sendTextToTTS()`, `flushTts()`.
- Speech delivery: EventEmitter callbacks, `speechEvents()` async iterator, or `events.mode: 'both'`.
- Typed `VoiceAgentConfig` — VAD (energy + optional Silero), STT/TTS provider selection, barge-in toggles.
- `wireVoiceAgentToDataChannel()` / `forwardVoiceAgentSpeechToDataChannel()` for browser control channels.
- Live vendor presets and opt-in live tests (`VOICE_LIVE_TEST=1`).

#### `@node-webrtc-rust/helpers` (new package)

- `SessionPod` — one signaling entry point, many concurrent sessions on one Node process.
- `VoiceAgentSessionHost` — per `client-*` peer: `RTCPeerConnection` + `VoiceAgent`, auto cleanup on disconnect.
- `VOICE_AGENT_SERVER_PEER_ID`, PCM kick-frame helpers (`createKickFrame`, frame size constants).

#### Rust — speech & vendors

- `crates/speech` — orchestration, event bus, pipeline traits, Silero VAD, barge-in buffer flush.
- Vendor crates: `vendor-openai`, `vendor-deepgram`, `vendor-elevenlabs`, `vendor-google`, `vendor-cartesia`, `vendor-assemblyai`, `vendor-mock`, `vendor-sherpa-onnx`.
- NAPI: `JsVoiceAgent` — attach, STT/TTS I/O, speech events to Node.

#### WebRTC SDK

- `RTCPeerConnection.addTransceiver`, `getTransceivers` / `getSenders` / `getReceivers`.
- `RTCRtpSender.replaceTrack`, `RTCPeerConnection.removeTrack`.
- `RemoteAudioTrack.readSample()` — inbound Opus → PCM.
- P1 parity: `getStats`, `setConfiguration`, `restartIce`, ICE/signaling state change events, data-channel backpressure.
- `createOffer` / `createAnswer` options wired to native ICE/SDP.

#### Conference / mixer

- Per-listener routing matrix on `MixGraph`.

#### Examples

| Example                              | Description                                                                  |
| ------------------------------------ | ---------------------------------------------------------------------------- |
| `voice-agent`                        | CLI loopback — mock/live vendors, callback/stream/barge-in modes             |
| `voice-agent-browser`                | Browser mic + Node `VoiceAgent`, DataChannel speech events & TTS control     |
| `voice-agent-local-sherpa`           | On-device Sherpa STT/TTS, model download scripts, roundtrip & barge-in demos |
| `voice-agent-multi-session-pod`      | Many sessions on one server via `SessionPod`                                 |
| `peer-connection` `start:parity`     | Transceivers, stats, config, replaceTrack tour                               |
| `audio-cosine` `start:replace-track` | `replaceTrack` + `readSample` demo                                           |

Shared helpers under `examples/shared/`: vendor presets, Sherpa model catalogs, PCM conventions (re-exported from `@node-webrtc-rust/helpers/pcm`).

#### Tests

- SDK: voice callback, stream, e2e, live vendor, control-channel bridge tests.
- Speech: VAD barge-in and TTS injection tests.
- CI: `verify-native-binding-surface.mjs` for NAPI export drift.

### Changed

- **Project positioning** — README and SDK docs refocused on **agentic voice workloads** (phone bots, browser assistants, worker pods).
- **Worker scaling guidance** — multi-session pods (N calls per process); scale replicas by CPU/RAM.
- **VAD defaults** — energy VAD threshold tuning; optional Silero with dynamic ONNX Runtime install path documented.
- **STT gating** — gate hold defers `user_speech_final` until silence hold ends; pre-roll ring for gated STT.
- **`bargeIn.useVad`** — separate auto barge on VAD from manual `flushTts()`.
- **Examples** — expanded inline comments (track directions, PCM kick frames, negotiation order).
- **CI** — TypeScript build order: sdk core → signaling → full sdk → **helpers**; helpers included in dist cache.

### Fixed

- Cross-compiled `.node` verification by target triple.
- Native binding cache: compile on TS changes; precise cache key; copy host `.node` to platform name after PR compile.
- Linux **aarch64-gnu** CI: native arm64 runner, link without Zig.
- Silero VAD compile issues and energy VAD default behavior.
- TTS drain / barge-in race in voice pipeline.
- `toNativeConfig` always returns `JsRtcConfiguration`.
- ESLint in transceiver APIs and `.mjs` download scripts.

### Migration notes (from `0.2.1`)

1. **Rebuild native bindings** after upgrade:
   ```bash
   npm run build:native && npm run build:ts
   ```
2. **New imports** for voice apps:
   ```bash
   npm install @node-webrtc-rust/sdk @node-webrtc-rust/signaling @node-webrtc-rust/helpers
   ```
3. **Live vendors** need optional Rust `live` features + env vars — see `examples/shared/VOICE_VENDOR_REFERENCE.md`.
4. **Sherpa local STT/TTS** requires model download and `SHERPA_*_MODEL_PATH`.
5. **PCM helpers** — prefer `@node-webrtc-rust/helpers/pcm`.

### Pull requests

| PR                                                           | Summary                                                 |
| ------------------------------------------------------------ | ------------------------------------------------------- |
| [#8](https://github.com/akirilyuk/node-webrtc-rust/pull/8)   | WebRTC P1 parity, transceivers, mixer routing matrix    |
| [#10](https://github.com/akirilyuk/node-webrtc-rust/pull/10) | Cross-binding verify, README voice refocus              |
| [#11](https://github.com/akirilyuk/node-webrtc-rust/pull/11) | v0.3 voice: vendors, Sherpa, browser demos, live wiring |
| [#12](https://github.com/akirilyuk/node-webrtc-rust/pull/12) | CI: Linux arm64 native gnu link                         |
| [#14](https://github.com/akirilyuk/node-webrtc-rust/pull/14) | `@node-webrtc-rust/helpers`, multi-session pod example  |

---

## [0.2.1] — 2026-05-28

Conference MCU, browser-compatible WebRTC SDK, signaling package. See [`release/0.2.1`](https://github.com/akirilyuk/node-webrtc-rust/releases/tag/release%2F0.2.1).
