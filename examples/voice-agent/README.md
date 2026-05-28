# Voice agent example

Mock demos (`start:callback`, `start:stream`, `start:barge-in`) run without API keys.

## Live vendor manual tests

One npm script per supported cloud provider. Each checks required env vars before starting.

| Vendor | Command | Required env |
|--------|---------|--------------|
| OpenAI | `npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent` | `OPENAI_API_KEY` |
| Deepgram | `npm run start:live:deepgram --workspace=@node-webrtc-rust/example-voice-agent` | `DEEPGRAM_API_KEY`, `OPENAI_API_KEY` (TTS pairing) |
| ElevenLabs | `npm run start:live:elevenlabs --workspace=@node-webrtc-rust/example-voice-agent` | `ELEVENLABS_API_KEY`, `OPENAI_API_KEY` (STT pairing) |
| Cartesia | `npm run start:live:cartesia --workspace=@node-webrtc-rust/example-voice-agent` | `CARTESIA_API_KEY`, `OPENAI_API_KEY` |
| AssemblyAI | `npm run start:live:assemblyai --workspace=@node-webrtc-rust/example-voice-agent` | `ASSEMBLYAI_API_KEY`, `OPENAI_API_KEY` |
| Google | `npm run start:live:google --workspace=@node-webrtc-rust/example-voice-agent` | `GOOGLE_APPLICATION_CREDENTIALS` |

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

**Note:** Vendor HTTP/WS calls require native builds with live features enabled in the vendor crates. Until live wiring is merged, TTS/STT may return a vendor error naming the missing `--features live` build — the example still validates config, attach, and loopback plumbing.
