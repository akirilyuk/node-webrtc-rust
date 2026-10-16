# vendor-fishaudio

Fish Audio TTS only (no STT yet) for `@node-webrtc-rust/sdk` VoiceAgent. Default model: **Drama 3** (`drama-3`).

## Docs

|                | URL                                                                                                       |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| WebSocket TTS  | [tts-live](https://docs.fish.audio/api-reference/endpoint/websocket/tts-live)                             |
| HTTP TTS       | [text-to-speech](https://docs.fish.audio/api-reference/endpoint/openapi-v1/text-to-speech)                |
| Drama 3        | [drama-3](https://docs.fish.audio/features/text-to-speech/drama-3)                                        |
| Realtime guide | [realtime-streaming](https://docs.fish.audio/features/realtime-streaming)                                 |
| Models         | [models-overview](https://docs.fish.audio/developer-guide/models-pricing/models-overview)                 |
| Rate limits    | [pricing-and-rate-limits](https://docs.fish.audio/developer-guide/models-pricing/pricing-and-rate-limits) |

## Environment

| Variable        | Use                                                                                                    |
| --------------- | ------------------------------------------------------------------------------------------------------ |
| `FISH_API_KEY`  | Fish Audio API key (`Authorization: Bearer`)                                                           |
| `FISH_VOICE_ID` | Voice `reference_id` used when `tts.voice` is unset, empty or `default`. Unset: `reference_id` omitted |

`tts.endpoint` replaces the API origin (default `https://api.fish.audio`). The WebSocket URL is derived from it (`https` to `wss`, `http` to `ws`).

## Streaming decisions (TTS)

| Choice        | What we implemented                                                              | Why                                                                                                                  |
| ------------- | -------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Default       | WS `/v1/tts/live`, MessagePack frames (`start`, `text`, `flush`, `stop`)         | Documented streaming path; audio reaches the sink while Fish is still synthesizing.                                  |
| Fallback      | HTTP `POST /v1/tts` (chunked body), only while no audio reached the sink         | Documented non-WS path. After audio started, a retry would play the start of the utterance twice, so the error wins. |
| Audio format  | `format: "pcm"`, `sample_rate: 24000`, mono s16le, upsampled x2 to 48 kHz stereo | 48 kHz is not documented for PCM. Same approach as the OpenAI and Groq vendors.                                      |
| Latency       | `latency: "balanced"`                                                            | Documented as the lowest time to first audio, meant for voice agents.                                                |
| Normalization | `normalize: true`                                                                | Documented request field.                                                                                            |
| Model         | `model` header, validated against the catalog before anything is sent            | Fish silently falls back to `s2.1-pro` on a missing or misspelled `model`. Unknown ids are a config error.           |
| Voice         | `reference_id` from `tts.voice`, else `FISH_VOICE_ID`, else omitted              | `reference_id` is nullable in the docs.                                                                              |
| Bounds        | connect 10 s, first audio 15 s, idle between chunks 10 s, HTTP total 60 s        | Every wait has a named error (`fishaudio ws first audio timed out after 15000 ms`).                                  |
| Cancel        | Sink cancel closes the socket (WS) or drops the body (HTTP)                      | Barge-in stops reading at the next frame.                                                                            |
| Sink contract | Every PCM chunk goes to the sink once, as it arrives                             | The returned chunk is the whole utterance for the phrase cache and is never re-sent to the sink.                     |

Only these request fields are sent: `text`, `format`, `sample_rate`, `latency`, `normalize`, `reference_id`. Request text and audio are never logged and never appear in error messages.

### Drama 3 text

Direction tags (`[nervous, quick]`), sound tags (`[small laugh]`) and paired tags (`<whisper>...</whisper>`) go in `text` exactly as written. The text is passed through untouched.

### Concurrency

Fish limits concurrent requests per tier: 5 (Starter), 15 (Elevated), 50 (High Volume). There is no client-side limiter; a request over the limit surfaces Fish's error.

STT: this vendor is TTS-only for now. Pair it with another STT provider.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-fishaudio
cargo test -p node-webrtc-rust-vendor-fishaudio --features live   # includes the mock-server tests
```
