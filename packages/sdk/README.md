# @node-webrtc-rust/sdk

TypeScript SDK for **agentic voice workloads** and WebRTC in Node.js — backed by a Rust native engine ([NAPI-RS](https://napi.rs) + [webrtc-rs](https://github.com/webrtc-rs/webrtc)).

Build phone bots, browser voice assistants, and session workers where **Node runs your LLM and tools**, and **Rust owns media timing** (VAD, barge-in, STT/TTS vendors, outbound PCM at 20 ms cadence).

```bash
npm install @node-webrtc-rust/sdk @node-webrtc-rust/signaling @node-webrtc-rust/helpers
```

## Exports

| Import path                        | Purpose                                                                                         |
| ---------------------------------- | ----------------------------------------------------------------------------------------------- |
| **`@node-webrtc-rust/sdk/voice`**  | **VoiceAgent** — VAD, barge-in, STT/TTS vendors, speech events — [VOICE-API.md](./VOICE-API.md) |
| `@node-webrtc-rust/sdk`            | W3C-style `RTCPeerConnection`, tracks, data channels                                            |
| `@node-webrtc-rust/sdk/conference` | Conference room control plane (MCU mixing)                                                      |

**WebRTC parity vs browser:** [`docs/webrtc-api-parity.md`](../../docs/webrtc-api-parity.md)

---

## Voice agent — build agentic workloads

Import **`@node-webrtc-rust/sdk/voice`** when you need a conversational loop without reimplementing PCM timing or vendor HTTP/WebSocket clients in Node.

**API reference:** [VOICE-API.md](./VOICE-API.md) (exports, speech events, data-channel bridge) · [VOICE-VAD-AND-BARGE-IN.md](./VOICE-VAD-AND-BARGE-IN.md) (tuning)

### Problem this solves

| Without VoiceAgent                          | With VoiceAgent                                                 |
| ------------------------------------------- | --------------------------------------------------------------- |
| Manual Opus decode + frame alignment        | Inbound loop via `RemoteAudioTrack.readSample()`                |
| Roll your own VAD / interrupt handling      | Configurable VAD + atomic TTS flush on barge-in                 |
| STT/TTS SDK calls in Node + glue to WebRTC  | Rust vendor adapters; Node gets text events + `sendTextToTTS()` |
| Race between user speech and agent playback | Native buffer flush **before** `barge_in` reaches JS            |

### Pipeline B (recommended)

STT final text **up** to Node → your LLM → TTS text **down** to Rust → outbound WebRTC track:

```typescript
import { LocalAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

// 1. Connect WebRTC (browser, SIP gateway, or another Node peer)
const pc = new RTCPeerConnection({ iceServers: [...] })
const agentTrack = new LocalAudioTrack('agent', 'session-1')
await pc.addTrack(agentTrack)

// Resolve tracks after negotiation (see examples/voice-agent/)
let userTrack: RemoteAudioTrack
pc.ontrack = (e) => { if (e.track.kind === 'audio') userTrack = e.track as RemoteAudioTrack }

// 2. Configure one VoiceAgent per conversation (VAD preset = good defaults for phone bots)
import { VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'

const agent = new VoiceAgent({
  vad: VOICE_AGENT_VAD_PRESET,
  stt: { provider: 'deepgram', model: 'nova-2', language: 'en' },
  tts: { provider: 'openai', model: 'tts-1', voice: 'alloy' },
  events: { mode: 'both' },
})

// 3. Bind tracks — one attach() = one session
await agent.attach({ inboundTrack: userTrack, outboundTrack: agentTrack })
await agent.start()

// 4. Agent loop — your business logic stays in TypeScript
agent.on('user_speech_final', async (event) => {
  const stream = await myLLM.chat(event.text!)
  for await (const token of stream) {
    await agent.sendTextToTTS(token)
  }
})

agent.on('barge_in', () => {
  cancelLLMStream()
  // flushTts already ran in Rust if bargeIn.flushTts === true
})
```

### Attach model

`attach({ inboundTrack, outboundTrack })` binds **one peer connection session**:

| Track           | Direction                         | Used for                        |
| --------------- | --------------------------------- | ------------------------------- |
| `inboundTrack`  | User → agent (`RemoteAudioTrack`) | VAD + STT (`readSample` loop)   |
| `outboundTrack` | Agent → user (`LocalAudioTrack`)  | TTS PCM after `sendTextToTTS()` |

Multiple concurrent callers = multiple `VoiceAgent` instances (one per WebRTC connection). Use [`@node-webrtc-rust/helpers`](../../helpers/README.md) `SessionPod` to run many sessions on one server process.

### Server helpers (`@node-webrtc-rust/helpers`)

| Helper                  | Use when                                                                               |
| ----------------------- | -------------------------------------------------------------------------------------- |
| `SessionPod`            | One signaling entry point, many concurrent sessions (orchestrator assigns `sessionId`) |
| `VoiceAgentSessionHost` | One signaling room; auto-spawns one agent per `client-*` peer                          |
| `createKickFrame`       | Prime outbound RTP before TTS PCM                                                      |

Runnable demo: [`examples/voice-agent-multi-session-pod`](../../examples/voice-agent-multi-session-pod/README.md).

### Configuration reference

```typescript
interface VoiceAgentConfig {
  vad?: {
    enabled?: boolean // default true
    provider?: 'energy' | 'silero' // default energy; silero needs custom build + local ONNX Runtime — see VOICE-VAD-AND-BARGE-IN.md
    threshold?: number // energy ~0.05–0.2; silero probability ~0.5
    minSpeechDurationMs?: number
    minSilenceDurationMs?: number
    speechPadMs?: number // pre-roll ring capacity (ms), default 300
    sampleRate?: 8000 | 16000
    bargeIn?: {
      enabled?: boolean // master switch (default true)
      useVad?: boolean // auto barge on VAD SpeechStart (default true); false = flushTts() only
      flushTts?: boolean // clear TTS buffer when barge-in runs (default true)
      requireSttPartial?: boolean // while agent TTS plays: barge only after STT partial (default true)
      minSttPartialChars?: number // min partial length for semantic barge (default 2)
      agentPlaybackGuardMs?: number // optional ms to ignore VAD barge after TTS starts (default 0)
    }
    gateStt?: boolean // only feed STT while gate is open (default false)
    gateSttOpenOnPending?: boolean // also open gate during VAD pending speech (default true)
    sttGateHoldMs?: number // keep feeding STT after speech end (default 1000)
  }
  stt?: {
    provider: 'openai' | 'deepgram' | 'google' | 'assemblyai' | 'local-sherpa' | 'mock'
    model?: string
    modelPath?: string // local-sherpa: ONNX model directory
    language?: string
    apiKey?: string // or env var — see vendor table
  }
  tts?: {
    provider: 'openai' | 'elevenlabs' | 'google' | 'cartesia' | 'local-sherpa' | 'mock'
    model?: string
    modelPath?: string // local-sherpa: Piper/VITS directory
    voice?: string
    apiKey?: string
  }
  events?: {
    mode?: 'callback' | 'stream' | 'both' // default 'both'
  }
}
```

### STT/TTS vendors

Providers are **mix-and-match** per session. **Official API docs:** [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../../examples/shared/VOICE_VENDOR_REFERENCE.md)

| Provider       | STT | TTS | Typical env var                                                         | API docs                                                                                                                       |
| -------------- | --- | --- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `openai`       | ✓   | ✓   | `OPENAI_API_KEY`                                                        | [STT](https://platform.openai.com/docs/guides/speech-to-text) · [TTS](https://platform.openai.com/docs/guides/text-to-speech)  |
| `deepgram`     | ✓   | —   | `DEEPGRAM_API_KEY`                                                      | [Live streaming](https://developers.deepgram.com/docs/live-streaming-audio)                                                    |
| `elevenlabs`   | —   | ✓   | `ELEVENLABS_API_KEY`                                                    | [TTS API](https://elevenlabs.io/docs/api-reference/text-to-speech/convert)                                                     |
| `cartesia`     | —   | ✓   | `CARTESIA_API_KEY`                                                      | [TTS bytes](https://docs.cartesia.ai/api-reference/tts/bytes)                                                                  |
| `assemblyai`   | ✓   | —   | `ASSEMBLYAI_API_KEY`                                                    | [Streaming STT](https://www.assemblyai.com/docs/speech-to-text/streaming)                                                      |
| `google`       | ✓   | ✓   | `GOOGLE_APPLICATION_CREDENTIALS`                                        | [STT](https://cloud.google.com/speech-to-text/docs) · [TTS](https://cloud.google.com/text-to-speech/docs)                      |
| `local-sherpa` | ✓   | ✓   | `SHERPA_STT_MODEL_PATH`, `SHERPA_TTS_MODEL_PATH`, `SHERPA_STT_LANGUAGE` | [Sherpa-ONNX](https://k2-fsa.github.io/sherpa/onnx/) · [Models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models) |
| `mock`         | ✓   | ✓   | _(none — use for CI/local)_                                             | —                                                                                                                              |

Example pairings when a vendor only supports one direction:

```typescript
// Deepgram listen + OpenAI speech
stt: { provider: 'deepgram', model: 'nova-2', language: 'en' },
tts: { provider: 'openai', model: 'tts-1', voice: 'alloy' },

// AssemblyAI STT + ElevenLabs TTS
stt: { provider: 'assemblyai', model: 'universal-streaming-english' },
tts: { provider: 'elevenlabs', model: 'eleven_multilingual_v2', voice: '...' },
```

API keys via `apiKey` in config or env vars. Never logged or returned in speech events.

#### Local Sherpa-ONNX (`local-sherpa`) — free on-device STT + TTS

**Recommended when privacy and speech latency matter.** User speech is transcribed and agent replies synthesized on **your server** with Sherpa-ONNX — no cloud STT/TTS API keys or network round-trips.

Requires downloaded **Zipformer transducer** (STT) and **Piper/VITS** (TTS) directories.

| Variable                | Required      | Purpose                                                                  |
| ----------------------- | ------------- | ------------------------------------------------------------------------ |
| `SHERPA_STT_MODEL_PATH` | **Yes** (STT) | Path to extracted Zipformer STT directory                                |
| `SHERPA_TTS_MODEL_PATH` | **Yes** (TTS) | Path to Piper/VITS directory (`tokens.txt`, `*.onnx`, `espeak-ng-data/`) |
| `SHERPA_STT_LANGUAGE`   | No            | `stt.language` tag (inferred from path when omitted)                     |
| `SHERPA_TTS_SPEAKER`    | No            | Piper speaker id for `tts.voice` (default `0`)                           |

Automatic downloads (from repo root):

```bash
npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
export SHERPA_STT_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06"
export SHERPA_TTS_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low"
```

Supported languages, bundle names, and unavailable locales (Hindi, Portuguese, Italian): [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../../examples/shared/VOICE_VENDOR_REFERENCE.md#local-sherpa-onnx--multilingual-models). Full walkthrough: [`examples/voice-agent-local-sherpa/README.md`](../../examples/voice-agent-local-sherpa/README.md).

```typescript
stt: {
  provider: 'local-sherpa',
  language: process.env.SHERPA_STT_LANGUAGE ?? 'en',
  modelPath: process.env.SHERPA_STT_MODEL_PATH,
},
tts: {
  provider: 'local-sherpa',
  modelPath: process.env.SHERPA_TTS_MODEL_PATH,
  voice: process.env.SHERPA_TTS_SPEAKER ?? '0',
},
```

### Speech events

Two layers — **fast VAD** vs **text-bearing STT**:

| Event                          | Source                            | Use in your agent                             |
| ------------------------------ | --------------------------------- | --------------------------------------------- |
| `user_speaking_start`          | VAD `SpeechStart`                 | User began talking (inbound voice activity)   |
| `user_speaking_end`            | VAD + `sttGateHoldMs` (`gateStt`) | End-of-utterance hint (not first short pause) |
| `user_speech_partial`          | STT                               | Live captions, prefetch                       |
| `user_speech_final`            | STT                               | **Primary LLM turn trigger**                  |
| `agent_speaking_start` / `end` | TTS playback                      | UI / state machine                            |
| `barge_in`                     | Barge-in path (see below)         | Cancel agent TTS (after optional flush)       |
| `error`                        | Any                               | Vendor or pipeline failure                    |

### VAD and barge-in

**Full guide:** [`VOICE-VAD-AND-BARGE-IN.md`](./VOICE-VAD-AND-BARGE-IN.md) — **energy vs Silero**, weight/comparison, use cases, defaults, when to tune.

**Quick start:** use `VOICE_AGENT_VAD_PRESET` (or omit `vad` and only set `gateStt: true` if you stream STT). Defaults: 250 ms min speech, 500 ms min silence, **`requireSttPartial: true`** (barge during agent TTS only after STT recognizes words), `sttGateHoldMs` 1000. See [VOICE-VAD-AND-BARGE-IN.md](./VOICE-VAD-AND-BARGE-IN.md#semantic-barge-in-requiresttpartial-default-true) and [STT flow fine-tuning](./VOICE-VAD-AND-BARGE-IN.md#stt-flow-fine-tuning-gatestt).

| Export                    | Use                                              |
| ------------------------- | ------------------------------------------------ |
| `VOICE_AGENT_VAD_PRESET`  | Recommended — `gateStt: true` + default barge-in |
| `DEFAULT_VOICE_AGENT_VAD` | Rust defaults (`gateStt: false`)                 |

Barge-in requires `vad.enabled` and `bargeIn.useVad` (both default `true`). With **`requireSttPartial: true`** (preset default), interrupt during **agent TTS** waits for a qualifying `user_speech_partial` before `barge_in` — coughs and tones alone do not cut playback. Set `requireSttPartial: false` for immediate energy-VAD barge (legacy). Set `useVad: false` and call `flushTts()` for manual interrupt only.

**Callback delivery:**

```typescript
agent.on('user_speech_final', (event) => {
  /* event.text */
})
agent.off('user_speech_final', handler)
```

**Stream delivery:**

```typescript
for await (const event of agent.speechEvents()) {
  switch (event.type) {
    case 'user_speech_final':
      await handleTurn(event.text!)
      break
    case 'barge_in':
      await cancelTurn()
      break
  }
}
```

Set `events.mode: 'both'` to use handlers and the iterator on the same session.

### Browser client over DataChannel

When the agent runs on Node and the user connects from a browser, mirror speech events and accept TTS requests on an `RTCDataChannel`:

```typescript
import {
  VOICE_CONTROL_CHANNEL_LABEL,
  wireVoiceAgentToDataChannel,
} from '@node-webrtc-rust/sdk/voice'

const dc = pc.createDataChannel(VOICE_CONTROL_CHANNEL_LABEL, { ordered: true })
dc.onopen = () => {
  wireVoiceAgentToDataChannel(agent, dc)
}
```

- **Server → client:** `{ type: 'speech_event', event: 'user_speech_final', text: '…' }` (STT, VAD, `barge_in`, …)
- **Client → server:** `{ type: 'speak', text: 'Hello' }` → `agent.sendTextToTTS(text)`

Full browser + Node demo: [`examples/voice-agent-browser`](../../examples/voice-agent-browser/README.md).

**Live vendors:** default is mock. Set `VOICE_VENDOR=openai|deepgram|elevenlabs|cartesia|assemblyai|google` and the vendor’s API keys, then run `npm run start:live:<vendor> --workspace=@node-webrtc-rust/example-voice-agent-browser`. See that README for env vars and pairing notes.

### API summary

| Method                                    | Description                                                          |
| ----------------------------------------- | -------------------------------------------------------------------- |
| `attach({ inboundTrack, outboundTrack })` | Bind one PC session                                                  |
| `start()`                                 | Start VAD/STT pipeline and inbound PCM loop                          |
| `stop()`                                  | Stop pipeline; ends `speechEvents()` iterator                        |
| `sendTextToTTS(text)`                     | Synthesize via TTS vendor → outbound track                           |
| `flushTts()`                              | Clear pending TTS (also used when `bargeIn.flushTts: false`)         |
| `on(type, fn)` / `off`                    | Callback handlers                                                    |
| `speechEvents()`                          | Async generator of all speech events                                 |
| `wireVoiceAgentToDataChannel(agent, dc)`  | Forward speech events + handle `{ type: 'speak' }` on a data channel |

### Examples and live vendor testing

From the repo root (after `npm run setup`):

| Command                                                                                                      | Purpose                                        |
| ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------- |
| `npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent`                                   | Mock vendors, `on()` handlers                  |
| `npm run start:stream --workspace=...`                                                                       | Mock vendors, `speechEvents()`                 |
| `npm run start:barge-in --workspace=...`                                                                     | VAD + `flushTts`                               |
| `npm run start --workspace=@node-webrtc-rust/example-voice-agent-browser`                                    | Browser mic + DataChannel events + TTS control |
| `OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent-browser`  | Browser demo with live OpenAI STT/TTS          |
| `npm run start:live:deepgram` / `elevenlabs` / … `--workspace=@node-webrtc-rust/example-voice-agent-browser` | Live browser demo per vendor (see README)      |
| `OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=...`                                            | Live OpenAI preset                             |
| `npm run start:live:deepgram` / `elevenlabs` / …                                                             | Per-vendor manual tests                        |

See [`examples/voice-agent/README.md`](../../examples/voice-agent/README.md) for env vars and track-direction notes.

**SDK live tests** (opt-in):

```bash
VOICE_LIVE_TEST=1 VOICE_LIVE_OPENAI=1 OPENAI_API_KEY=sk-... \
  npm run test --workspace=@node-webrtc-rust/sdk -- voice-live
```

Use **`mock`** providers for CI and learning the API. Live vendor HTTP/WebSocket runs in Rust `vendor-*` crates; default builds use stubs until optional `live` features are enabled.

---

## WebRTC core

Low-level transport for attaching `VoiceAgent` to browsers, telephony gateways, or Node peers:

```typescript
import { RTCPeerConnection, LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import { SignalingServer, SignalingClient, autoNegotiate } from '@node-webrtc-rust/signaling'
```

**Recent APIs:** `addTransceiver`, `getTransceivers` / `getSenders` / `getReceivers`, `RTCRtpSender.replaceTrack`, `removeTrack`, `RemoteAudioTrack.readSample`, `getStats`, `setConfiguration`.

Peer connection quick start: [root README](../../README.md#webrtc-core-and-conference).

PCM conventions (kick frame, 20 ms frames): [`@node-webrtc-rust/helpers/pcm`](../../packages/helpers/README.md#pcm-kick-frame) (also re-exported from [`examples/shared/pcm-streaming.ts`](../../examples/shared/pcm-streaming.ts)).

---

## Conference rooms

Multi-participant MCU mixing — separate from single-session `VoiceAgent`, but useful for group voice apps.

Conference APIs live under **`@node-webrtc-rust/sdk/conference`**. TypeScript manages room lifecycle and mute policy; Rust mixes Opus/PCM in `crates/conference`.

### Control plane vs data plane

| Layer         | Package / crate                  | Responsibility                              |
| ------------- | -------------------------------- | ------------------------------------------- |
| Control plane | `sdk/conference`                 | Room lifecycle, mute/kick, signaling bridge |
| Signaling     | `@node-webrtc-rust/signaling`    | WebSocket SDP/ICE relay                     |
| Data plane    | `bindings` → `crates/conference` | Peer connections, mixer graph               |

### Quick start

```typescript
import { createServer } from 'http'

import { ConferenceServer } from '@node-webrtc-rust/sdk/conference'
import { SignalingServer } from '@node-webrtc-rust/signaling'

const PORT = 3000
const httpServer = createServer()
const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
await signaling.listen(PORT)

const conference = new ConferenceServer()
conference.attachSignaling({ url: `ws://127.0.0.1:${PORT}/ws` })

conference.on('participant-joined', ({ roomId, participantId }) => {
  console.log(`joined ${participantId} in ${roomId}`)
})

await conference.createRoom('demo', {
  maxParticipants: 16,
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
})
```

See [`examples/conference-room`](../../examples/conference-room/) for a browser demo.

### Mute modes

| Mode                  | API                                                      | Effect                                        |
| --------------------- | -------------------------------------------------------- | --------------------------------------------- |
| **Global mute**       | `muteParticipant(id, { scope: 'global' })`               | Target excluded from **all** listeners' mixes |
| **Listener mute**     | `muteParticipant(id, { scope: 'listener', listenerId })` | Only `listenerId` stops hearing `id`          |
| **Room-wide silence** | `setMixingEnabled(false)`                                | Mixer outputs silence; mute state preserved   |

### Authentication (production)

The library does not enforce roles. Gate admin actions in your app:

| Action                          | Recommended policy         |
| ------------------------------- | -------------------------- |
| Global mute / kick / mixing off | Admin / moderator only     |
| Listener-scoped mute            | Owning client (preference) |

### Events

`ConferenceServer` emits: `room-created`, `room-destroyed`, `participant-joined`, `participant-left`, `participant-kicked`, `participant-muted`, `mixing-enabled-changed`, `error`.

---

## Debug logging

```bash
WEBRTC_DEBUG=1 node your-agent.js
```

Traces SDK, bindings, and Rust core on stderr with `[webrtc-debug]` prefix.

---

## License

MIT
