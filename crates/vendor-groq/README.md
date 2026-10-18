# vendor-groq

Groq cloud STT/TTS for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|     | URL                                                                                  |
| --- | ------------------------------------------------------------------------------------ |
| STT | [console.groq.com/docs/speech-to-text](https://console.groq.com/docs/speech-to-text) |
| TTS | [console.groq.com/docs/text-to-speech](https://console.groq.com/docs/text-to-speech) |

## Environment

| Variable       | Use                               |
| -------------- | --------------------------------- |
| `GROQ_API_KEY` | Bearer token for both STT and TTS |

Optional `config.apiKey` overrides the env var.

## STT models (`config.model`)

| Model                    | Transport                                                    |
| ------------------------ | ------------------------------------------------------------ |
| `whisper-large-v3-turbo` | File `POST …/audio/transcriptions` on VAD finalize (default) |
| `whisper-large-v3`       | Same                                                         |

## Streaming decisions

**STT — file only.** Groq’s published STT docs are multipart `POST …/audio/transcriptions` (Whisper file/url). There is **no** documented live listen WebSocket. We do not invent one. After VoiceAgent VAD `finalize_utterance`, this crate POSTs the utterance WAV and emits a single final (`user_speech_partial` never fires).

**TTS — streamed WAV body.** Groq TTS is `POST …/audio/speech`. The API reference lists `response_format` as "Supported formats are `flac, mp3, mulaw, ogg, wav`": there is no raw PCM format, so the request asks for `wav` and the body is read as it arrives (see Transport).

Translations endpoint is out of scope.

## TTS models (`config.model`)

| Model                             | Transport                                       |
| --------------------------------- | ----------------------------------------------- |
| `canopylabs/orpheus-v1-english`   | `POST …/audio/speech` → WAV, streamed (default) |
| `canopylabs/orpheus-arabic-saudi` | Same                                            |

`config.voice` is passed through (Orpheus voices such as `troy`, `hannah` per Groq TTS docs); unset means `troy`. Orpheus may answer a 4xx that says the model's terms must be accepted: that is an account setting in the Groq console, and the error text from Groq is in the message.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-groq --lib
cargo test -p node-webrtc-rust-vendor-groq --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
```

The live checks against the real API (`tests/live_test.rs`) and the latency bench (`tests/live_bench.rs`) are described under Transport.

## Transport

Config: `tts.transport` and `stt.transport`: `auto` (default) or `http`. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

Both legs are plain HTTP requests.

**TTS streams the WAV body.** Groq's speech page says the endpoint "defaults to `wav`"; the API reference lists `flac, mp3, mulaw, ogg, wav` and a `sample_rate` of 8000 to 48000 (default 48000). Only `wav` can be played without a decoder, so the WAV header is parsed incrementally (`src/wav.rs`): `RIFF`/`WAVE`, then chunks until `data`, taking the rate from `fmt `. Only 16-bit PCM mono is accepted, anything else fails with a named error (`Groq TTS WAV is not 16-bit PCM …`, `… has 2 channels …`). The `RIFF` and `data` sizes are ignored, so a streamed WAV with a placeholder size works. The `data` payload goes to the sink as it arrives, each chunk once, an odd trailing byte carried to the next chunk. Whatever rate the `fmt ` chunk names (8000 to 48000 Hz) is converted to the agent's 48 kHz stereo by `MonoToStereo48k` (`crates/speech/src/pcm.rs`, linear interpolation, continuous across chunks; 48000 is a passthrough); a rate outside that range is an error. A body that errors or ends early after audio is an error with no retry, and a body without audio is an error (`Groq TTS returned no audio`). Bounds: first audio 15 s (request and headers included), 10 s between chunks, 60 s for the whole request.

**STT is a finals-only batch upload.** `emits_partials()` is `false`: the file endpoint returns one final per utterance. A failed request (network error, non-2xx, timeout, an answer without the documented `text` field) is an `Err` from `poll_transcript`; the STT actor turns it into a vendor error event for the utterance. The status and Groq's error text are in the message, the key never is. An empty `text` is silence: no final and no error. Each finalize is an independent request, so the bench measures a second utterance on the same provider as `warm` (pooled connection).

`auto` resolves to the catalog default, which is today's behaviour: TTS `http`, STT `http`. Default measured in a later change.

HTTP requests use one pooled client per provider; HTTP/2 when the vendor negotiates it (ALPN), otherwise HTTP/1.1. Connect timeout 10 s, whole-request timeout 60 s, idle pool 90 s, TCP keepalive 30 s.

### Live tests and bench

```bash
# from node-webrtc-rust/
GROQ_API_KEY=... cargo test -p node-webrtc-rust-vendor-groq --features live --test live_test -- --nocapture --test-threads=1
NWR_BENCH_RUNS=3 GROQ_API_KEY=... cargo test -p node-webrtc-rust-vendor-groq --features live --test live_bench -- --nocapture --test-threads=1
```

Without the key they skip; `GROQ_LIVE_REQUIRED=1` turns a missing key into a panic that names it (CI sets it, `.github/workflows/vendor-live-groq.yml`; secret `GROQ_LIVE_KEY`). The STT test synthesizes its phrase with Groq TTS, so it needs the Orpheus terms accepted too. The bench runs through `vendor-bench.yml`. The mock-server tests run in PR integration (`cargo vendor-groq`).
