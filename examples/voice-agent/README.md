# Voice agent example

Runnable tours of `@node-webrtc-rust/sdk/voice` — mock demos need **no API keys**; live scripts
exercise each cloud vendor once you have credentials.

## Architecture (read this first)

```
┌─────────────┐   userOut RTP    ┌─────────────┐
│  user PC    │ ───────────────► │  agent PC   │
│  (simulated │                  │ VoiceAgent  │
│   caller)   │ ◄─────────────── │  host       │
└─────────────┘   agentOut RTP   └─────────────┘
                       ▲
                       │ sendTextToTTS() → TTS vendor → PCM
```

- **`agentInbound`** — attach as `inboundTrack` (user speech → VAD/STT)
- **`agentOut`** — attach as `outboundTrack` (agent TTS → WebRTC send)
- Do **not** swap these; `userInbound` is agent audio heard on the user side only.

Shared helpers and comments: `src/shared-loopback.ts`, `examples/shared/pcm-streaming.ts`.

**Official STT/TTS API docs:** [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md)

## Mock demos (CI-safe)

| Script | File | Teaches |
|--------|------|---------|
| `start:callback` | `src/callback.ts` | `agent.on()` event handlers |
| `start:stream` | `src/stream.ts` | `for await … speechEvents()` |
| `start:barge-in` | `src/barge-in.ts` | VAD + `bargeIn.flushTts` |

| Vendor | Command | Required env |
|--------|---------|--------------|
| OpenAI | `npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent` | `OPENAI_API_KEY` |
| Deepgram | `npm run start:live:deepgram --workspace=@node-webrtc-rust/example-voice-agent` | `DEEPGRAM_API_KEY`, `OPENAI_API_KEY` (TTS pairing) |
| ElevenLabs | `npm run start:live:elevenlabs --workspace=@node-webrtc-rust/example-voice-agent` | `ELEVENLABS_API_KEY`, `OPENAI_API_KEY` (STT pairing) |
| Cartesia | `npm run start:live:cartesia --workspace=@node-webrtc-rust/example-voice-agent` | `CARTESIA_API_KEY`, `OPENAI_API_KEY` |
| AssemblyAI | `npm run start:live:assemblyai --workspace=@node-webrtc-rust/example-voice-agent` | `ASSEMBLYAI_API_KEY`, `OPENAI_API_KEY` |
| Google | `npm run start:live:google --workspace=@node-webrtc-rust/example-voice-agent` | `GOOGLE_APPLICATION_CREDENTIALS` |

### Official API documentation

| Vendor | STT docs | TTS docs |
|--------|----------|----------|
| OpenAI | [Speech to text](https://platform.openai.com/docs/guides/speech-to-text) | [Text to speech](https://platform.openai.com/docs/guides/text-to-speech) |
| Deepgram | [Live streaming](https://developers.deepgram.com/docs/live-streaming-audio) | — (STT-only; demo pairs OpenAI TTS) |
| ElevenLabs | — (TTS-only; demo pairs OpenAI STT) | [TTS API](https://elevenlabs.io/docs/api-reference/text-to-speech/convert) |
| Cartesia | — | [TTS bytes](https://docs.cartesia.ai/api-reference/tts/bytes) |
| AssemblyAI | [Streaming STT](https://www.assemblyai.com/docs/speech-to-text/streaming) | — |
| Google Cloud | [Speech-to-Text](https://cloud.google.com/speech-to-text/docs) | [Text-to-Speech](https://cloud.google.com/text-to-speech/docs) |

Full table with default models: [`VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md)

Optional overrides:

- `ELEVENLABS_VOICE_ID` — ElevenLabs voice id (default: Rachel)
- `CARTESIA_VOICE_ID` — Cartesia voice id
- `VOICE_VENDOR=openai` — alternative to passing vendor as CLI arg

### What the live demo does

1. Builds a bidirectional loopback WebRTC session (agent ↔ user legs)
2. Starts `VoiceAgent` with the vendor preset
3. Streams a 440 Hz tone on the user leg for 3s (STT/VAD exercise)
4. Calls `sendTextToTTS()` with a vendor-specific phrase
5. Logs speech events to the console

### SDK live tests (same credentials)

From repo root, after `npm run setup`:

```bash
VOICE_LIVE_TEST=1 \
VOICE_LIVE_OPENAI=1 \
OPENAI_API_KEY=sk-... \
npm run test --workspace=@node-webrtc-rust/sdk -- voice-live
```

Enable one vendor at a time with `VOICE_LIVE_<VENDOR>=1` (e.g. `VOICE_LIVE_DEEPGRAM=1`) plus that vendor’s required env vars from the table above.

For local on-device STT (no cloud keys), use [`voice-agent-local-sherpa`](../voice-agent-local-sherpa/README.md).
