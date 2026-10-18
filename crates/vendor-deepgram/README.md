# vendor-deepgram

Deepgram listen (STT) and speak (TTS) for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|               | URL                                                                               |
| ------------- | --------------------------------------------------------------------------------- |
| Live listen   | [Live streaming audio](https://developers.deepgram.com/docs/live-streaming-audio) |
| Listen models | [Models](https://developers.deepgram.com/docs/models)                             |
| Aura speak    | [Speak](https://developers.deepgram.com/reference/text-to-speech/speak)           |
| Flux TTS      | [Flux TTS overview](https://developers.deepgram.com/docs/flux-tts/overview)       |

## Environment

| Variable           | Use                       |
| ------------------ | ------------------------- |
| `DEEPGRAM_API_KEY` | Bearer for listen + speak |

`config.apiKey` overrides the env var.

### Endpoint override

`TtsConfig.endpoint` (for example `http://127.0.0.1:<port>`) replaces `https://api.deepgram.com` for TTS. REST becomes `{endpoint}/v1/speak?…` (or `/v2/speak` for Flux) and WebSocket uses the same base with `https` mapped to `wss` and `http` to `ws`. Unset keeps the default host. The local mock-server tests use this. STT is unaffected.

## Streaming decisions (STT)

| Choice              | What we implemented                                            | Why                                                                                                                           |
| ------------------- | -------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Live listen WS      | `wss://api.deepgram.com/v1/listen` with `interim_results=true` | Official live-streaming guide. Partials (`is_final=false`) become `user_speech_partial`.                                      |
| Models              | `nova-2` (default), `nova-3` only                              | Those ids are on the documented listen model list.                                                                            |
| **Not** Flux listen | `flux-*` / `flux-general` rejected by `validate_listen_model`  | Flux is documented for **speak** (`/v2/speak`), not as a listen model on `/v1/listen`. We do not invent a Flux listen socket. |

PCM: 16 kHz mono linear16 (VoiceAgent STT rate).

## Streaming decisions (TTS)

| Choice          | What we implemented                                        | Why                                                                                     |
| --------------- | ---------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Aura (`aura-*`) | REST + WS `…/v1/speak`                                     | Official Aura speak API. Progressive WS when VoiceAgent calls `synthesize_progressive`. |
| Flux (`flux-*`) | REST + WS `…/v2/speak` (`SpeechStarted`, `SpeechMetadata`) | Official Flux TTS. Same voice id cannot be sent to `/v1`.                               |
| Routing         | Prefix: `flux-*` → v2, `aura-*` → v1                       | Documented path split. Unknown prefixes fail config.                                    |

Default speak model: `aura-asteria-en`.

### Timeouts

Connect 10 s (also bounds each WS control send), first audio 15 s, between audio frames 10 s, REST total 60 s. A WebSocket that closes or errors before `Flushed` is an error, never a partial success.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-deepgram --lib
cargo test -p node-webrtc-rust-vendor-deepgram --features live --test mock_server_test
```

## Transport

Config: `tts.transport`: `auto` (default), `websocket` or `http`. `stt.transport`: `auto` or `websocket`. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

TTS `websocket` (`/v1/speak` or `/v2/speak` socket, audio chunks reach the sink as they arrive). TTS `http` (`POST /v1/speak`; the body is read in full before it is sent to the sink today, streaming comes in a later change). STT is always the `/v1/listen` WebSocket.

`auto` resolves to the catalog default, which is today's behaviour: TTS `websocket`, STT `websocket`. Default measured in a later change.
