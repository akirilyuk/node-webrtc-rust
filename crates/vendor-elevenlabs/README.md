# vendor-elevenlabs

ElevenLabs Scribe realtime STT and TTS for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

| | URL |
|---|---|
| Scribe realtime STT | [Speech to text realtime](https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime) |
| TTS HTTP stream | [Stream](https://elevenlabs.io/docs/api-reference/text-to-speech/stream) |
| TTS WebSocket | [Realtime TTS websockets](https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts) |

## Environment

| Variable | Use |
|---|---|
| `ELEVENLABS_API_KEY` | `xi-api-key` for STT + TTS |

`config.apiKey` overrides the env var. TTS `config.voice` is the ElevenLabs `voice_id`.

## Streaming decisions (STT)

| Choice | What we implemented | Why |
|---|---|---|
| Transport | `wss://api.elevenlabs.io/v1/speech-to-text/realtime` | Official Scribe v2 realtime API. |
| Model | `scribe_v2_realtime` only | Documented realtime model. File Scribe (`scribe_v2`) is **not** this socket and is rejected. |
| Commit | `commit_strategy=manual` + `input_audio_chunk` on `finalize_utterance` | Matches VoiceAgent VAD: we own utterance boundaries. |
| Events | `partial_transcript` → partial; `committed_transcript` → final | Official event names. |

## Streaming decisions (TTS)

| Choice | What we implemented | Why |
|---|---|---|
| Default progressive | HTTP `POST …/text-to-speech/{voice_id}/stream` (`pcm_48000`) | Official stream API; works for `eleven_multilingual_v2`, Flash, etc. |
| WebSocket `stream-input` | Implemented and **allowed** except `eleven_v3` | Official realtime TTS WS. |
| `eleven_v3` | Full-body `POST …/text-to-speech/{voice_id}` only | **Documented:** `stream-input` does not support `eleven_v3`. We do not send that model on the WS. |

Default TTS model: `eleven_multilingual_v2`.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-elevenlabs --lib
```
