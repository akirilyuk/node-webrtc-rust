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
cargo test -p node-webrtc-rust-vendor-deepgram --features live --test mock_server_test --test http_pool_test
```

## Transport

Config: `tts.transport`: `auto` (default), `websocket` or `http`. `stt.transport`: `auto` or `websocket`. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

TTS `websocket`: the `/v1/speak` (Aura) or `/v2/speak` (Flux) socket. Every binary message reaches the sink as it arrives, each chunk once. TTS `http`: `POST /v1/speak`; the response body is forwarded to the sink chunk by chunk as it arrives (`Response::chunk`), in 48 kHz stereo frames, each chunk once. A body that errors or ends early (reset, short `Content-Length`) is an error, never partial success, and nothing is retried after audio reached the sink. Deepgram documents the linear16 default container as `wav` ("When using the wav container, the response includes standard container metadata"), and the request keeps its query unchanged, so a canonical 44-byte RIFF/WAVE header at the start of the body is skipped. STT is always the `/v1/listen` WebSocket.

`auto` resolves to the catalog default, which is today's behaviour: TTS `websocket`, STT `websocket`. Default measured in a later change.

### WebSocket reuse across utterances (Aura)

One speak socket is kept per `DeepgramTts` and reused for the next utterance. Basis in the streaming TTS guide: "If you are building for conversational AI use cases where a human is talking to a TTS agent, a single websocket per conversation is required", text is queued with `Speak` and "you can obtain the corresponding audio by sending a `Flush` command", and "An active websocket has a 60-minute timeout period from the initial connection". The socket opens on the first utterance, is reused while open, is replaced after 55 minutes or when the server closed it (a failure before any audio reaches the sink may retry once on a fresh socket), and is never reused after an error or a cancel (a cancel sends the documented `Clear`, then closes). After audio reached the sink an error is returned and nothing is replayed. Flux (`/v2/speak`) keeps one socket per utterance: its end marker is `SpeechMetadata`, which is not documented as a per-flush marker. `Flush` is limited to 20 per 60 s per socket (docs), above any conversational turn rate.

### Connect bound

Every WebSocket connect (TTS and STT) is bounded at 10 s with the error `deepgram <tts|stt> websocket connect timed out after 10 s`.

### Live tests and bench

```bash
# from node-webrtc-rust/
DEEPGRAM_API_KEY=... cargo test -p node-webrtc-rust-vendor-deepgram --features live --test live_test -- --nocapture --test-threads=1
NWR_BENCH_RUNS=3 DEEPGRAM_API_KEY=... cargo test -p node-webrtc-rust-vendor-deepgram --features live --test live_bench -- --nocapture --test-threads=1
```

Without the key both skip; `DEEPGRAM_LIVE_REQUIRED=1` turns a missing key into a panic (CI sets it, `.github/workflows/vendor-live-deepgram.yml`). The bench runs through `vendor-bench.yml`.

HTTP requests use one pooled client per provider; HTTP/2 when the vendor negotiates it (ALPN), otherwise HTTP/1.1.
