# @node-webrtc-rust/sdk

TypeScript SDK for **agentic voice workloads** and WebRTC in Node.js — backed by a Rust native engine ([NAPI-RS](https://napi.rs) + [webrtc-rs](https://github.com/webrtc-rs/webrtc)).

Build phone bots, browser voice assistants, and session workers where **Node runs your LLM and tools**, and **Rust owns media timing** (VAD, barge-in, STT/TTS vendors, outbound PCM at 20 ms cadence).

```bash
npm install @node-webrtc-rust/sdk @node-webrtc-rust/signaling
```

## Exports

| Import path | Purpose |
| ----------- | ------- |
| **`@node-webrtc-rust/sdk/voice`** | **VoiceAgent** — VAD, barge-in, STT/TTS vendors, speech events |
| `@node-webrtc-rust/sdk` | W3C-style `RTCPeerConnection`, tracks, data channels |
| `@node-webrtc-rust/sdk/conference` | Conference room control plane (MCU mixing) |

**WebRTC parity vs browser:** [`docs/webrtc-api-parity.md`](../../docs/webrtc-api-parity.md)

---

## Voice agent — build agentic workloads

Import **`@node-webrtc-rust/sdk/voice`** when you need a conversational loop without reimplementing PCM timing or vendor HTTP/WebSocket clients in Node.

### Problem this solves

| Without VoiceAgent | With VoiceAgent |
| --- | --- |
| Manual Opus decode + frame alignment | Inbound loop via `RemoteAudioTrack.readSample()` |
| Roll your own VAD / interrupt handling | Configurable VAD + atomic TTS flush on barge-in |
| STT/TTS SDK calls in Node + glue to WebRTC | Rust vendor adapters; Node gets text events + `sendTextToTTS()` |
| Race between user speech and agent playback | Native buffer flush **before** `barge_in` reaches JS |

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

// 2. Configure one VoiceAgent per conversation
const agent = new VoiceAgent({
  vad: {
    enabled: true,
    threshold: 0.5,
    minSpeechDurationMs: 250,
    minSilenceDurationMs: 100,
    bargeIn: { enabled: true, flushTts: true },
    gateStt: false, // set true to only send PCM to STT during detected speech
  },
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

| Track | Direction | Used for |
| --- | --- | --- |
| `inboundTrack` | User → agent (`RemoteAudioTrack`) | VAD + STT (`readSample` loop) |
| `outboundTrack` | Agent → user (`LocalAudioTrack`) | TTS PCM after `sendTextToTTS()` |

Multiple concurrent callers = multiple `VoiceAgent` instances, each with its own attach context.

### Configuration reference

```typescript
interface VoiceAgentConfig {
  vad?: {
    enabled?: boolean              // default true
    provider?: 'silero'            // Silero when native built with silero-vad feature
    threshold?: number             // 0.0–1.0, default 0.5
    minSpeechDurationMs?: number
    minSilenceDurationMs?: number
    speechPadMs?: number
    sampleRate?: 8000 | 16000
    bargeIn?: {
      enabled?: boolean            // emit barge_in on user speech start
      flushTts?: boolean           // flush TTS buffer before event (default true)
    }
    gateStt?: boolean              // only feed STT during detected speech
  }
  stt?: {
    provider: 'openai' | 'deepgram' | 'google' | 'assemblyai' | 'mock'
    model?: string
    language?: string
    apiKey?: string                // or env var — see vendor table
  }
  tts?: {
    provider: 'openai' | 'elevenlabs' | 'google' | 'cartesia' | 'mock'
    model?: string
    voice?: string
    apiKey?: string
  }
  events?: {
    mode?: 'callback' | 'stream' | 'both'  // default 'both'
  }
}
```

### STT/TTS vendors

Providers are **mix-and-match** per session:

| Provider | STT | TTS | Typical env var |
| --- | --- | --- | --- |
| `openai` | ✓ | ✓ | `OPENAI_API_KEY` |
| `deepgram` | ✓ | — | `DEEPGRAM_API_KEY` |
| `elevenlabs` | — | ✓ | `ELEVENLABS_API_KEY` |
| `cartesia` | — | ✓ | `CARTESIA_API_KEY` |
| `assemblyai` | ✓ | — | `ASSEMBLYAI_API_KEY` |
| `google` | ✓ | ✓ | `GOOGLE_APPLICATION_CREDENTIALS` |
| `mock` | ✓ | ✓ | _(none — use for CI/local)_ |

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

### Speech events

Two layers — **fast VAD** vs **text-bearing STT**:

| Event | Source | Use in your agent |
| --- | --- | --- |
| `user_speaking_start` | VAD | Early interrupt; triggers barge-in |
| `user_speaking_end` | VAD | End-of-utterance hint |
| `user_speech_partial` | STT | Live captions, prefetch |
| `user_speech_final` | STT | **Primary LLM turn trigger** |
| `agent_speaking_start` / `end` | TTS playback | UI / state machine |
| `barge_in` | VAD + config | Cancel LLM/TTS pipeline |
| `error` | Any | Vendor or pipeline failure |

**Callback delivery:**

```typescript
agent.on('user_speech_final', (event) => { /* event.text */ })
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

### API summary

| Method | Description |
| --- | --- |
| `attach({ inboundTrack, outboundTrack })` | Bind one PC session |
| `start()` | Start VAD/STT pipeline and inbound PCM loop |
| `stop()` | Stop pipeline; ends `speechEvents()` iterator |
| `sendTextToTTS(text)` | Synthesize via TTS vendor → outbound track |
| `flushTts()` | Clear pending TTS (also used when `bargeIn.flushTts: false`) |
| `on(type, fn)` / `off` | Callback handlers |
| `speechEvents()` | Async generator of all speech events |

### Examples and live vendor testing

From the repo root (after `npm run setup`):

| Command | Purpose |
| --- | --- |
| `npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent` | Mock vendors, `on()` handlers |
| `npm run start:stream --workspace=...` | Mock vendors, `speechEvents()` |
| `npm run start:barge-in --workspace=...` | VAD + `flushTts` |
| `OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=...` | Live OpenAI preset |
| `npm run start:live:deepgram` / `elevenlabs` / … | Per-vendor manual tests |

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

PCM conventions (kick frame, 20 ms frames): [`examples/shared/pcm-streaming.ts`](../../examples/shared/pcm-streaming.ts).

---

## Conference rooms

Multi-participant MCU mixing — separate from single-session `VoiceAgent`, but useful for group voice apps.

Conference APIs live under **`@node-webrtc-rust/sdk/conference`**. TypeScript manages room lifecycle and mute policy; Rust mixes Opus/PCM in `crates/conference`.

### Control plane vs data plane

| Layer | Package / crate | Responsibility |
| ----- | ---------------- | -------------- |
| Control plane | `sdk/conference` | Room lifecycle, mute/kick, signaling bridge |
| Signaling | `@node-webrtc-rust/signaling` | WebSocket SDP/ICE relay |
| Data plane | `bindings` → `crates/conference` | Peer connections, mixer graph |

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

| Mode | API | Effect |
| ---- | --- | ------ |
| **Global mute** | `muteParticipant(id, { scope: 'global' })` | Target excluded from **all** listeners' mixes |
| **Listener mute** | `muteParticipant(id, { scope: 'listener', listenerId })` | Only `listenerId` stops hearing `id` |
| **Room-wide silence** | `setMixingEnabled(false)` | Mixer outputs silence; mute state preserved |

### Authentication (production)

The library does not enforce roles. Gate admin actions in your app:

| Action | Recommended policy |
| ------ | ------------------- |
| Global mute / kick / mixing off | Admin / moderator only |
| Listener-scoped mute | Owning client (preference) |

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
