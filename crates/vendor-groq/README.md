# vendor-groq

Groq cloud STT/TTS for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

| | URL |
|---|---|
| STT | [console.groq.com/docs/speech-to-text](https://console.groq.com/docs/speech-to-text) |
| TTS | [console.groq.com/docs/text-to-speech](https://console.groq.com/docs/text-to-speech) |

## Environment

| Variable | Use |
|---|---|
| `GROQ_API_KEY` | Bearer token for both STT and TTS |

Optional `config.apiKey` overrides the env var.

## STT models (`config.model`)

| Model | Transport |
|---|---|
| `whisper-large-v3-turbo` | File `POST …/audio/transcriptions` on VAD finalize (default) |
| `whisper-large-v3` | Same |

**Not streamed:** Groq docs describe file/url transcription only — no documented live WebSocket. No partial transcripts; `poll_transcript` waits while the multipart POST is in flight after `finalize_utterance`.

Translations endpoint is out of scope.

## TTS models (`config.model`)

| Model | Transport |
|---|---|
| `canopylabs/orpheus-v1-english` | Full-body `POST …/audio/speech` → WAV (default) |
| `canopylabs/orpheus-arabic-saudi` | Same |

`config.voice` is passed through (Orpheus voices such as `troy`, `hannah` per Groq TTS docs). No documented progressive SSE/chunked TTS — `synthesize_progressive` drains the full response then emits chunks.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-groq --lib
```

Live tests (optional): set `GROQ_API_KEY` — not required for CI unit/matrix tests.
