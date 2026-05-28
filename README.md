# node-webrtc-rust

[![Build](https://github.com/akirilyuk/node-webrtc-rust/actions/workflows/build.yml/badge.svg)](https://github.com/akirilyuk/node-webrtc-rust/actions/workflows/build.yml)

**Real-time voice agents in Node.js — WebRTC transport, Rust media timing, your LLM logic.**

[node-webrtc-rust](https://github.com/akirilyuk/node-webrtc-rust) is a native WebRTC stack for building **agentic voice workloads**: phone bots, browser voice assistants, and multi-tenant worker pods where Node runs business logic and Rust owns audio timing, VAD, barge-in, and TTS playback.

Install an npm package, load a prebuilt `.node` binary, attach a `VoiceAgent` to a peer connection, and wire `user_speech_final` → your LLM → `sendTextToTTS()` — without reimplementing PCM frame cadence, Opus decode, or vendor HTTP/WebSocket clients in TypeScript.

Unlike standalone media servers (Mediasoup, LiveKit), there is **no separate SFU cluster** — WebRTC, mixing, and voice pipelines run **in-process** beside your agent code.

---

## Table of contents

- [Why build voice agents here?](#why-build-voice-agents-here)
- [Agentic voice quick start](#agentic-voice-quick-start)
- [Voice pipeline architecture](#voice-pipeline-architecture)
- [STT/TTS vendors and config](#stttts-vendors-and-config)
- [Speech events and barge-in](#speech-events-and-barge-in)
- [Examples and manual vendor testing](#examples-and-manual-vendor-testing)
- [WebRTC core and conference](#webrtc-core-and-conference)
- [Packages](#packages)
- [Supported platforms](#supported-platforms)
- [Development](#development)
- [Debug logging](#debug-logging)
- [Releases](#releases)
- [WebRTC API parity](#webrtc-api-parity)
- [Roadmap](#roadmap)
- [License](#license)

---

## Why build voice agents here?

Building a production voice agent means solving three problems at once:

| Problem | Typical pain | node-webrtc-rust approach |
| --- | --- | --- |
| **Transport** | WebRTC ICE/SDP, Opus, jitter in Node | Browser-compatible `RTCPeerConnection` + native Rust RTP |
| **Media timing** | TTS/STT frame alignment, barge-in latency | `VoiceAgent` in Rust: VAD, TTS buffer, atomic flush |
| **Agent logic** | LLM, tools, RAG, billing | Stay in **your** TypeScript — events up, text down |

```mermaid
flowchart LR
  subgraph user [User]
    Mic[Mic / phone / browser]
  end

  subgraph rust [Rust data plane]
    WebRTC[WebRTC + Opus]
    VAD[VAD + barge-in]
    STT[STT vendor]
    TTS[TTS vendor]
    WebRTC --> VAD --> STT
    TTS --> WebRTC
  end

  subgraph node [Node control plane]
    Agent[Your agent loop]
    LLM[LLM / tools / RAG]
    STT -->|user_speech_final| Agent
    Agent --> LLM
    LLM -->|sendTextToTTS| TTS
  end

  Mic <-->|RTP| WebRTC
```

**What you implement in Node:** session config, LLM calls, tool use, persistence, auth.  
**What Rust handles:** inbound PCM loop, speech detection, vendor STT/TTS I/O, outbound PCM at 20 ms cadence, barge-in buffer flush before your callback runs.

One `VoiceAgent` instance binds to **one conversation** (one attached peer connection). Scale out with one agent per session/worker pod.

---

## Agentic voice quick start

Install the SDK (voice APIs are on the `/voice` subpath):

```bash
npm install @node-webrtc-rust/sdk @node-webrtc-rust/signaling
```

Minimal **Pipeline B** loop — STT text events up, your LLM, TTS text down:

```typescript
import { LocalAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

// After you have a connected PC and remote/local audio tracks from ontrack + addTrack:
const agent = new VoiceAgent({
  vad: {
    enabled: true,
    threshold: 0.5,
    bargeIn: { enabled: true, flushTts: true }, // native flush before barge_in event
  },
  stt: { provider: 'deepgram', model: 'nova-2', language: 'en' },
  tts: { provider: 'openai', model: 'tts-1', voice: 'alloy' },
  events: { mode: 'both' },
})

await agent.attach({
  inboundTrack: remoteUserTrack,   // user speech → VAD/STT
  outboundTrack: agentLocalTrack, // TTS → remote hears agent
})
await agent.start()

// Callback style — familiar EventEmitter pattern
agent.on('user_speech_final', async (event) => {
  const reply = await myLLM(event.text!)
  for await (const chunk of reply) {
    await agent.sendTextToTTS(chunk) // streams to outbound track
  }
})

// Stream style — single async iterator for all speech events
void (async () => {
  for await (const event of agent.speechEvents()) {
    if (event.type === 'barge_in') await cancelLLMStream()
  }
})()
```

Use **`mock`** STT/TTS providers for local dev and CI (no API keys). See [Examples](#examples-and-manual-vendor-testing).

Full SDK reference: [`packages/sdk/README.md`](packages/sdk/README.md#voice-agent-build-agentic-workloads).

---

## Voice pipeline architecture

```
Inbound RTP (user) ──► RemoteAudioTrack.readSample()
                              │
                              ▼
                    ┌─────────────────────┐
                    │  VAD (energy/Silero) │──► user_speaking_start/end
                    │  barge-in flush      │──► barge_in
                    └──────────┬──────────┘
                               ▼
                    ┌─────────────────────┐
                    │  STT vendor adapter  │──► user_speech_partial/final
                    └─────────────────────┘

sendTextToTTS(text) ──► TTS vendor adapter ──► TtsPlaybackBuffer
                                                      │
                                                      ▼
                              LocalAudioTrack.writeSample() ──► Outbound RTP (agent)
```

| Layer | Location | Role |
| --- | --- | --- |
| Orchestration | `crates/speech` | Config, event bus, VAD, barge-in, TTS queue |
| Vendors | `crates/vendor-*` | OpenAI, Deepgram, ElevenLabs, Google, Cartesia, AssemblyAI, mock |
| Node API | `@node-webrtc-rust/sdk/voice` | `VoiceAgent`, typed config, callbacks + `speechEvents()` |
| Transport | `@node-webrtc-rust/sdk` | `RTCPeerConnection`, `LocalAudioTrack`, `RemoteAudioTrack` |

**Frame format:** 48 kHz stereo, 16-bit PCM, 20 ms frames (3 840 bytes) on the WebRTC track path. VAD resamples to mono 16 kHz internally.

---

## STT/TTS vendors and config

STT and TTS providers are **independently configurable** — mix vendors per session:

| Provider | STT | TTS | Env var(s) |
| --- | --- | --- | --- |
| `openai` | ✓ | ✓ | `OPENAI_API_KEY` |
| `deepgram` | ✓ | — | `DEEPGRAM_API_KEY` |
| `elevenlabs` | — | ✓ | `ELEVENLABS_API_KEY` |
| `cartesia` | — | ✓ | `CARTESIA_API_KEY` |
| `assemblyai` | ✓ | — | `ASSEMBLYAI_API_KEY` |
| `google` | ✓ | ✓ | `GOOGLE_APPLICATION_CREDENTIALS` |
| **`local-sherpa`** | ✓ | — | `SHERPA_MODEL_PATH`, `SHERPA_LANGUAGE` |
| `mock` | ✓ | ✓ | _(none — CI/local)_ |

Pass `apiKey` in config or rely on env vars. Keys are never logged or returned in events.

### Free local STT (`local-sherpa`)

For production voice agents that handle **sensitive audio** or need **lower STT latency**, prefer **`local-sherpa`**: Sherpa-ONNX runs on your worker CPU, so user speech is **not** sent to third-party STT APIs and you skip cloud STT network round-trips. Models are free to download; no STT API key required.

```bash
npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
export SHERPA_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-2023-06-26"
npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Multilingual bundles and per-language scripts: [`examples/voice-agent-local-sherpa/README.md`](examples/voice-agent-local-sherpa/README.md). Cloud STT vendors in the table above remain available when you need vendor-specific models or locales without a local bundle.

Live HTTP/WebSocket calls live in Rust `vendor-*` crates (SDK-first). Default CI builds use stub adapters; enable per-crate `live` features when wiring production vendor calls.

---

## Speech events and barge-in

| Event | Source | When to use in your agent |
| --- | --- | --- |
| `user_speaking_start` | VAD | Fast interrupt signal; pairs with barge-in |
| `user_speaking_end` | VAD | End-of-utterance hint before STT final |
| `user_speech_partial` | STT | Live captions, early LLM prefetch |
| `user_speech_final` | STT | **Primary turn trigger** for LLM |
| `agent_speaking_start` / `end` | TTS playback | UI/state machine |
| `barge_in` | VAD + config | User interrupted agent — cancel LLM/TTS |
| `error` | Any | Vendor or pipeline failure |

**Barge-in** is two independent toggles under `vad.bargeIn`:

| `enabled` | `flushTts` | Behavior |
| --- | --- | --- |
| `true` | `true` (default) | Native TTS buffer flush **first**, then `barge_in` event |
| `true` | `false` | Emit `barge_in` only — TTS keeps playing until you call `flushTts()` |
| `false` | * | No barge-in event, no native flush |

**Delivery:** `events.mode` = `callback` | `stream` | `both` (handlers + `speechEvents()` async iterator).

---

## Examples and manual vendor testing

```bash
npm run setup   # once: deps + native .node + TS build
```

| Example | Command | Teaches |
| --- | --- | --- |
| **voice-agent-local-sherpa** | `download-model` + `npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa` | **Free on-device STT** — privacy-friendly, no cloud STT API; browser mic → Sherpa |
| **voice-agent** callback | `npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent` | `agent.on()` handlers, mock vendors |
| **voice-agent** stream | `npm run start:stream --workspace=...` | `for await … speechEvents()` |
| **voice-agent** barge-in | `npm run start:barge-in --workspace=...` | VAD + `flushTts` |
| **voice-agent** live OpenAI | `OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=...` | Real vendor config + loopback |
| **voice-agent** live * | `start:live:deepgram` / `elevenlabs` / `cartesia` / `assemblyai` / `google` | Per-vendor credentials |

Inline comments in [`examples/voice-agent/`](examples/voice-agent/) explain track directions (`agentInbound` vs `agentOut`), event modes, and vendor pairing. See [`examples/voice-agent/README.md`](examples/voice-agent/README.md).

SDK live tests (opt-in):

```bash
VOICE_LIVE_TEST=1 VOICE_LIVE_OPENAI=1 OPENAI_API_KEY=sk-... \
  npm run test --workspace=@node-webrtc-rust/sdk -- voice-live
```

Other WebRTC demos (peer connection, conference MCU): [`examples/README.md`](examples/README.md).

---

## WebRTC core and conference

### Why node-webrtc-rust vs standalone SFU?

| | node-webrtc-rust | Standalone SFU/MCU |
|---|---|---|
| **Deployment** | npm install | Separate server cluster |
| **Voice agents** | In-process `VoiceAgent` + your Node loop | Custom bridge to media server |
| **API surface** | W3C-style `RTCPeerConnection` | Proprietary client SDK |
| **Conference mixing** | In-process Rust MCU | Remote media server |
| **Best for** | Embedded agents, session workers | Large hosted rooms |

### Quick start — peer connection

```bash
npm install @node-webrtc-rust/sdk @node-webrtc-rust/signaling
```

```typescript
import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { SignalingServer, SignalingClient, autoNegotiate } from '@node-webrtc-rust/signaling'

const server = new SignalingServer({ port: 8080 })
await server.listen()

const pc1 = new RTCPeerConnection({
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
})
const sig1 = new SignalingClient({ url: 'ws://localhost:8080', room: 'demo' })
autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
await sig1.connect()

const dc = pc1.createDataChannel('chat')
dc.onopen = () => dc.send('Hello from Peer 1!')
```

### Quick start — conference room

Multi-participant MCU with personalized mixes (everyone else, never self):

```typescript
import { ConferenceServer } from '@node-webrtc-rust/sdk/conference'
import { SignalingServer } from '@node-webrtc-rust/signaling'

const conference = new ConferenceServer()
conference.attachSignaling({ url: 'ws://127.0.0.1:8080/ws' })
await conference.createRoom('demo', {
  maxParticipants: 16,
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
})
```

See [`packages/sdk/README.md`](packages/sdk/README.md) for conference mute modes and events.

### Features (summary)

- **WebRTC core:** ICE, DTLS, DataChannels, Unified Plan transceivers, `RemoteAudioTrack.readSample()`
- **Conference (v0.2):** Rust MCU, exclude-self mixing, mute matrix, kick/admin APIs

---

## Architecture

```mermaid
flowchart TB
  subgraph node [Node.js — agent control plane]
    VoiceSDK["sdk/voice VoiceAgent"]
    SDK["sdk RTCPeerConnection"]
    LLM[Your LLM + tools]
    VoiceSDK --> LLM
  end

  subgraph native [Rust — data plane]
    Bind["bindings"]
    Speech["speech VAD STT TTS"]
    Core["core WebRTC"]
    Mixer["mixer"]
    ConfCrate["conference"]
    Bind --> Speech
    Bind --> Core
    Bind --> ConfCrate
    ConfCrate --> Mixer
  end

  SDK --> Bind
  VoiceSDK --> Bind
  Speech --> Core
```

---

## Packages

| Package | npm | Role |
| --- | --- | --- |
| [`@node-webrtc-rust/sdk`](packages/sdk) | TypeScript | WebRTC API + [`/voice`](packages/sdk/README.md#voice-agent-build-agentic-workloads) + [`/conference`](packages/sdk/README.md#conference-rooms) |
| [`@node-webrtc-rust/bindings`](packages/bindings) | Native | NAPI addon — peer connections, tracks, VoiceAgent, conference |
| [`@node-webrtc-rust/signaling`](packages/signaling) | TypeScript | WebSocket signaling server, client, auto-negotiate |

Platform-specific binding packages (`@node-webrtc-rust/bindings-darwin-arm64`, etc.) ship with releases.

---

## Supported platforms

Prebuilt `.node` binaries are published for:

| OS | Architecture | Triple |
| --- | --- | --- |
| macOS | Apple Silicon (M1+) | `aarch64-apple-darwin` |
| macOS | Intel | `x86_64-apple-darwin` |
| Linux | x64 glibc | `x86_64-unknown-linux-gnu` |
| Linux | x64 musl (Alpine) | `x86_64-unknown-linux-musl` |
| Linux | arm64 glibc | `aarch64-unknown-linux-gnu` |
| Windows | x64 MSVC | `x86_64-pc-windows-msvc` |

Node.js **≥ 18** required.

---

## Development

### Prerequisites

- [Rust](https://rustup.rs) (stable)
- Node.js ≥ 18, npm ≥ 9

### Clone and build

```bash
git clone https://github.com/akirilyuk/node-webrtc-rust.git
cd node-webrtc-rust
npm run setup   # install deps, build native .node, build TS packages
```

Or step by step:

```bash
npm run install:all
npm run build:native   # host-only debug .node (~10s after cache warm)
npm run build:ts
```

Use release builds (`cd packages/bindings && npm run build:local`) before release-sensitive tests. Reserve `npm run build:all` in bindings for CI / publish verification only.

### Tests

```bash
# Everything: Rust workspace + npm workspaces (sdk, signaling)
npm run test:all

# Rust only
npm run test:rust

# TypeScript / Vitest only (sdk, signaling)
npm run test:ts
```

`test:all` runs `cargo test --workspace` (core, mixer, conference, bindings compile) and `npm test` in every workspace that defines a test script. Requires a built `.node` — use `npm run build:native` first if needed.

TURN integration (optional, skipped by default):

```bash
docker compose -f docker-compose.test.yml up -d
TURN_AVAILABLE=1 npm test --workspace=@node-webrtc-rust/sdk
docker compose -f docker-compose.test.yml down
```

CI builds all platform targets using GitHub Actions. See **[`scripts/ci/README.md`](scripts/ci/README.md)** for pipeline diagrams, path filters, and caching.

Linux builds and tests use a prebuilt container image (`ghcr.io/akirilyuk/node-webrtc-rust/ci-build:latest`) with Node, Rust, Zig, and CMake — rebuild it by pushing to the `ci` branch (see [`docker/ci/Dockerfile`](docker/ci/Dockerfile) and [`.github/workflows/ci-image.yml`](.github/workflows/ci-image.yml)). macOS and Windows jobs use native runners.

Before opening a PR, mirror CI locally to save Actions minutes:

```bash
npm run ci:verify:linux          # Linux napi cross-builds (Docker, same as CI)
npm run ci:verify:checks:docker  # format, lint, typecheck, cargo test, npm test
npm run ci:verify                # both
```

---

## Debug logging

Trace function calls and events across Rust core, NAPI bindings, SDK, signaling, and conference layers:

```bash
WEBRTC_DEBUG=1 node your-app.js
```

Accepted values: `1`, `true`, or `yes` (case-insensitive). Output uses the `[webrtc-debug]` prefix on stderr (Rust) and `console.error` (TypeScript):

```bash
WEBRTC_DEBUG=1 node your-app.js 2>&1 | grep '\[webrtc-debug\]'
```

Per-connection override:

```typescript
const pc = new RTCPeerConnection({
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
  debug: true,
})
```

When `debug` is set on the config object, it overrides the `WEBRTC_DEBUG` environment variable for that process.

---

## Releases

Full guide: [`scripts/RELEASE.md`](scripts/RELEASE.md)

### CI release (all platforms)

Merge to **`main`**, then push a **`release/*`** tag. GitHub Actions builds every target, runs tests, publishes to npm, and opens a GitHub Release.

```bash
git checkout main && git pull
git tag release/0.2.0
git push origin release/0.2.0
```

Tag examples: `release/0.2.0`, `release/0.2.0-beta.1`, `release/0.2.0-rc.1`. The part after `release/` is the npm version.

Requires repository secret **`NPM_TOKEN`**. Linux jobs use the CI image built from the **`ci`** branch (`ghcr.io/akirilyuk/node-webrtc-rust/ci-build:latest`).

### Local release

| Script | Use when |
| --- | --- |
| [`scripts/release-local.sh`](scripts/release-local.sh) | Publish from your machine for **one platform** (host `.node` only) |
| [`scripts/release-publish.sh`](scripts/release-publish.sh) | macOS: build Linux + Darwin locally; supply Windows `.node` separately |

```bash
# Host-only (fast)
./scripts/release-local.sh 0.2.0 "$NPM_TOKEN" --dry-run

# All platforms you can build on macOS (+ prebuilt Windows)
export NPM_TOKEN=...
npm run release:publish -- 0.2.0
```

After a local publish, commit version bumps and optionally push the same `release/x.y.z` tag for GitHub Release metadata.

---

## WebRTC API parity

The SDK mirrors browser **WebRTC 1.0** where it matters for Node↔browser audio and data channels. Full gap analysis (supported / partial / missing) lives in **[`docs/webrtc-api-parity.md`](docs/webrtc-api-parity.md)** — update that doc when adding or changing public APIs.

High-level: ICE/SDP, data channels, P0–P1 parity, and Unified Plan transceivers are in place for Node↔browser audio. **Video**, **simulcast**, **DTMF**, and **`MediaDevices`** are planned for **v0.4**; see roadmap below.

---

## Roadmap

| Version | Focus |
| --- | --- |
| **v0.1.0** | PeerConnection, DataChannels, audio tracks, STUN/TURN |
| **v0.2.x** | Conference MCU, API parity P0–P1, Unified Plan transceivers |
| **v0.3.0** | **Voice agents:** `VoiceAgent`, VAD, barge-in, six STT/TTS vendors, speech event stream |
| **v0.3.x** | Live vendor HTTP/WS, Silero VAD default, Pipeline A (realtime vendor WS) |
| **v0.4.0** | Video, simulcast, DTMF, conference video compositing |

---

## License

MIT
