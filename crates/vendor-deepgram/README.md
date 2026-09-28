# vendor-deepgram

Deepgram listen (STT) and speak (TTS) for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

| | URL |
|---|---|
| Live listen | [Live streaming audio](https://developers.deepgram.com/docs/live-streaming-audio) |
| Listen models | [Models](https://developers.deepgram.com/docs/models) |
| Aura speak | [Speak](https://developers.deepgram.com/reference/text-to-speech/speak) |
| Flux TTS | [Flux TTS overview](https://developers.deepgram.com/docs/flux-tts/overview) |

## Environment

| Variable | Use |
|---|---|
| `DEEPGRAM_API_KEY` | Bearer for listen + speak |

`config.apiKey` overrides the env var.

## Streaming decisions (STT)

| Choice | What we implemented | Why |
|---|---|---|
| Live listen WS | `wss://api.deepgram.com/v1/listen` with `interim_results=true` | Official live-streaming guide. Partials (`is_final=false`) become `user_speech_partial`. |
| Models | `nova-2` (default), `nova-3` only | Those ids are on the documented listen model list. |
| **Not** Flux listen | `flux-*` / `flux-general` rejected by `validate_listen_model` | Flux is documented for **speak** (`/v2/speak`), not as a listen model on `/v1/listen`. We do not invent a Flux listen socket. |

PCM: 16 kHz mono linear16 (VoiceAgent STT rate).

## Streaming decisions (TTS)

| Choice | What we implemented | Why |
|---|---|---|
| Aura (`aura-*`) | REST + WS `…/v1/speak` | Official Aura speak API. Progressive WS when VoiceAgent calls `synthesize_progressive`. |
| Flux (`flux-*`) | REST + WS `…/v2/speak` (`SpeechStarted`, `SpeechMetadata`) | Official Flux TTS. Same voice id cannot be sent to `/v1`. |
| Routing | Prefix: `flux-*` → v2, `aura-*` → v1 | Documented path split. Unknown prefixes fail config. |

Default speak model: `aura-asteria-en`.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-deepgram --lib
```
