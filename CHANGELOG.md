# Changelog

All notable changes to this project are documented here.

Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased]

_No changes yet._

---

## [0.4.1] — 2026-06-02

**Compare:** [`release/0.4.0…release/0.4.1`](https://github.com/akirilyuk/node-webrtc-rust/compare/release/0.4.0...release/0.4.1)

Patch release — same bits as 0.4.0; fixes Release workflow on self-hosted runners.

### Fixed

- **Release workflow** — add workspace `chown` prepare step before checkout on **Plan native builds** (quality job leaves root-owned `node_modules` / `target`, which broke `actions/checkout` on the shared runner).
- **Release workflow** — skip build/test/stage jobs when the plan job fails (avoids integration test without binding artifacts).

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
