# Voice Agent — Browser + Node

Mixed **browser client** and **Node server** example for agentic voice apps.

## Architecture

```
Browser                         Node server
───────                         ───────────
getUserMedia ──WebRTC audio──►  VoiceAgent inbound (VAD + STT)
                                │
                                ▼
Event log ◄── DataChannel ◄── speech events (partial, final, barge_in, …)
   │
   └── { type: 'speak', text } ──► sendTextToTTS() ──► agent outbound track ──► browser <audio>
```

- **Signaling:** WebSocket (`@node-webrtc-rust/signaling`) on the same HTTP port as the static page.
- **Control channel:** label `voice-control` (see `VOICE_CONTROL_CHANNEL_LABEL` in `@node-webrtc-rust/sdk/voice`).
- **SDK helper:** `wireVoiceAgentToDataChannel(agent, channel)` forwards speech events and handles inbound `speak` messages.

## Run (mock — default)

No API keys. Mock STT emits deterministic transcripts after ~1 s of speech; mock TTS plays synthesized audio on the agent track.

```bash
npm run start --workspace=@node-webrtc-rust/example-voice-agent-browser
```

Open [http://localhost:3001](http://localhost:3001) (override with `PORT=`).

1. Allow microphone access.
2. Click **Connect**.
3. Speak — mock STT sends `user_speech_partial` / `user_speech_final` over the data channel.
4. Type text and click **Speak** — mock TTS plays on the agent audio track.
5. Click **Speak long reply**, then talk over the playback to see **`barge_in`** in the event log.

## Unmock: live cloud vendors

By default the server uses **mock** STT/TTS (`provider: 'mock'`). To use real cloud APIs:

1. Set **`VOICE_VENDOR`** to one of the supported vendor ids (see table below).
2. Export that vendor’s **required env vars** before starting the server.
3. Open the browser page and connect — your **microphone** feeds live STT; TTS from the form uses the configured cloud TTS vendor.

Presets live in [`examples/shared/voice-vendor-presets.ts`](../shared/voice-vendor-presets.ts) (same as `examples/voice-agent`).

### Supported vendors

| Vendor | `VOICE_VENDOR` | STT | TTS | Required env | npm script |
|--------|----------------|-----|-----|--------------|------------|
| OpenAI | `openai` | OpenAI | OpenAI | `OPENAI_API_KEY` | `start:live:openai` |
| Deepgram | `deepgram` | Deepgram | OpenAI (pairing) | `DEEPGRAM_API_KEY`, `OPENAI_API_KEY` | `start:live:deepgram` |
| ElevenLabs | `elevenlabs` | OpenAI (pairing) | ElevenLabs | `ELEVENLABS_API_KEY`, `OPENAI_API_KEY` | `start:live:elevenlabs` |
| Cartesia | `cartesia` | OpenAI (pairing) | Cartesia | `CARTESIA_API_KEY`, `OPENAI_API_KEY` | `start:live:cartesia` |
| AssemblyAI | `assemblyai` | AssemblyAI | OpenAI (pairing) | `ASSEMBLYAI_API_KEY`, `OPENAI_API_KEY` | `start:live:assemblyai` |
| Google Cloud | `google` | Google | Google | `GOOGLE_APPLICATION_CREDENTIALS` | `start:live:google` |

**Pairing note:** Not every vendor supports both STT and TTS in this SDK. Demos pair STT-only vendors with OpenAI TTS (or Google with Google). In production you can mix `VoiceAgentConfig.stt` and `.tts` freely.

### Live commands (all vendors)

From the **repo root**, after `npm run setup`:

```bash
# OpenAI — STT + TTS on one key
OPENAI_API_KEY=sk-... \
  npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent-browser

# Deepgram STT + OpenAI TTS
DEEPGRAM_API_KEY=... OPENAI_API_KEY=sk-... \
  npm run start:live:deepgram --workspace=@node-webrtc-rust/example-voice-agent-browser

# ElevenLabs TTS + OpenAI STT
ELEVENLABS_API_KEY=... OPENAI_API_KEY=sk-... \
  npm run start:live:elevenlabs --workspace=@node-webrtc-rust/example-voice-agent-browser

# Cartesia TTS + OpenAI STT
CARTESIA_API_KEY=... OPENAI_API_KEY=sk-... \
  npm run start:live:cartesia --workspace=@node-webrtc-rust/example-voice-agent-browser

# AssemblyAI STT + OpenAI TTS
ASSEMBLYAI_API_KEY=... OPENAI_API_KEY=sk-... \
  npm run start:live:assemblyai --workspace=@node-webrtc-rust/example-voice-agent-browser

# Google Cloud Speech + TTS (Application Default Credentials)
GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json \
  npm run start:live:google --workspace=@node-webrtc-rust/example-voice-agent-browser
```

Equivalent without npm scripts — set `VOICE_VENDOR` yourself:

```bash
VOICE_VENDOR=openai OPENAI_API_KEY=sk-... \
  npm run start --workspace=@node-webrtc-rust/example-voice-agent-browser
```

### Optional voice overrides

| Env | Used when |
|-----|-----------|
| `ELEVENLABS_VOICE_ID` | `VOICE_VENDOR=elevenlabs` (default: Rachel) |
| `CARTESIA_VOICE_ID` | `VOICE_VENDOR=cartesia` |
| `GOOGLE_API_KEY` | Alternative to ADC for some Google setups |

### Verify live mode

On startup the server logs the active pipeline, for example:

```
Voice pipeline: OpenAI — STT=openai, TTS=openai
```

In mock mode you will see:

```
Voice pipeline: mock (no API keys) — STT=mock, TTS=mock
Using mock STT/TTS. Set VOICE_VENDOR and API keys for live vendors — see README.
```

### Live native builds

Vendor HTTP/WebSocket calls run in Rust `vendor-*` crates. Default CI builds may use stub adapters. If TTS/STT fails with a message about `--features live`, rebuild the native addon with live features enabled for that vendor (see crate docs / follow-ups in the repo).

### Custom STT/TTS mix

To use a config not covered by presets, edit `src/resolve-voice-config.ts` or pass explicit `stt` / `tts` in code — the browser page and DataChannel protocol stay the same.

## Data channel protocol

| Direction | Payload |
|-----------|---------|
| Client → server | `{ "type": "speak", "text": "Hello" }` |
| Server → client | `{ "type": "speech_event", "event": "user_speech_final", "text": "…" }` |

Server events mirror `SpeechEvent` types: `user_speaking_start`, `user_speech_partial`, `user_speech_final`, `agent_speaking_start`, `agent_speaking_end`, `barge_in`, `error`.

## Related

- Node-only loopback demos: [`examples/voice-agent/`](../voice-agent/README.md) (same live presets, no browser)
- SDK bridge API: [`packages/sdk/src/voice/speech-event-bridge.ts`](../../packages/sdk/src/voice/speech-event-bridge.ts)
- SDK live tests: `VOICE_LIVE_TEST=1 VOICE_LIVE_OPENAI=1 … npm run test --workspace=@node-webrtc-rust/sdk -- voice-live`
