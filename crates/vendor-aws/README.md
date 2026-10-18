# vendor-aws

AWS Transcribe streaming STT and Polly TTS for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|                      | URL                                                                                           |
| -------------------- | --------------------------------------------------------------------------------------------- |
| Transcribe streaming | [Streaming setup](https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html) |
| Polly                | [SynthesizeSpeech](https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html)     |

## Environment

Standard AWS SDK credential chain:

| Variable                | Use                                       |
| ----------------------- | ----------------------------------------- |
| `AWS_ACCESS_KEY_ID`     | IAM access key                            |
| `AWS_SECRET_ACCESS_KEY` | Secret key                                |
| `AWS_REGION`            | Region for Transcribe streaming and Polly |

Optional session token via `AWS_SESSION_TOKEN`.

## Streaming decisions (STT)

| Choice         | What we implemented                                          | Why                                                                                                          |
| -------------- | ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------ |
| Transport      | AWS SDK `StartStreamTranscription` (PCM 16 kHz)              | Official [Transcribe streaming](https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html). |
| Partials       | `TranscriptEvent` `IsPartial` + `Alternatives[0].Transcript` | Documented event JSON.                                                                                       |
| `config.model` | Language code (`en-US` default)                              | Streaming examples use language codes, not a separate model id. Allowlist in `src/matrix.rs`.                |

## Streaming decisions (TTS)

| Choice                               | What we implemented                                                                 | Why                                                                                                                                                                                                                                                                                                     |
| ------------------------------------ | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Transport                            | Polly `SynthesizeSpeech`, `Engine::Neural`, PCM 16 kHz                              | Official [SynthesizeSpeech](https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html). `AudioStream` forwarded chunk by chunk.                                                                                                                                                             |
| Voices                               | Sample neural ids: `Joanna` (default), `Matthew`, `Amy`, `Brian`, `Ruth`, `Stephen` | Documented neural voices. Unknown ids fail validation.                                                                                                                                                                                                                                                  |
| **Not** `StartSpeechSynthesisStream` | —                                                                                   | Official stream API exists but **only the `generative` engine** is supported. Neural voices on that call error. Progressive TTS for `Joanna` is **not** documented on `SynthesizeSpeech`. Generative stream is a second product (HTTP/2 `TextEvent` / `AudioEvent`), not an upgrade of the neural path. |

## rustls CryptoProvider (native bindings)

The AWS SDK default HTTPS client enables rustls `aws-lc-rs`. Other vendors enable rustls `ring` (reqwest / `tokio-tungstenite`). rustls 0.23 panics if both features are linked and no process-level `CryptoProvider` is installed.

NAPI `module_init` in `packages/bindings/src/runtime.rs` installs the **ring** provider before any TLS/DTLS work. Do not remove that install when changing AWS crate features.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-aws --lib
cargo test -p node-webrtc-rust-vendor-aws --features live --lib --test mock_server_test --test vendor_test
```

The mock-server tests build the Polly client with fixed fake credentials and a local endpoint, so they never read `~/.aws` or the AWS variables of the environment. The live checks against the real API (`tests/live_test.rs`) and the latency bench (`tests/live_bench.rs`) are described under Transport.

## Transport

Config: `tts.transport` and `stt.transport`: `auto` (default) or `http`. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

Polly and Transcribe streaming go through the AWS SDK over HTTP.

**Polly streams the `AudioStream`.** The response element is documented as "Stream containing the synthesized speech", and the SDK hands it out as a byte stream. Each chunk goes to the sink as it arrives (`output.audio_stream.try_next()` loop, not `collect()`), once, an odd trailing byte carried to the next chunk. `pcm` is "audio/pcm in a signed 16-bit, 1 channel (mono), little-endian format" and its only valid rates are "8000" and "16000" (default 16000), so the 16 kHz samples are repeated three times to the agent's 48 kHz stereo (no resampler exists for TTS). A stream that errors after audio is an error with no retry, and an empty stream is an error (`Polly returned no audio`). Bounds: first audio 15 s (request included), 10 s between chunks, plus the 60 s SDK operation timeout of the request.

**Transcribe emits partials.** The docs say each `Results` object has an `IsPartial` field and "If this field is true, your transcription segment is not yet complete". `IsPartial: true` is surfaced as `SttTranscript::Partial`, `false` as `Final`, so `emits_partials()` stays `true`. A stream that ends with an error after start (reset, service exception) is an `Err` from the next `poll_transcript`, and the STT actor turns it into a vendor error event; before this the reader task dropped the error and the agent heard silence. The stream error path is covered by unit tests on the forwarding function; a fake HTTP/2 event-stream server for the SDK path does not exist, so the end-to-end case is left to the live test. Transcribe has no per-utterance finalize and shares one stream across utterances, so the bench measures `cold` only.

`auto` resolves to the catalog default, which is today's behaviour: TTS `http`, STT `http`. Default measured in a later change.

Each provider keeps one SDK client (built on first use), and the SDK pools connections per client; HTTP/2 when the service negotiates it (Transcribe streaming is an HTTP/2 event stream), otherwise HTTP/1.1. There is no `reqwest` here. `last_transport()` reports `h2` for Transcribe and `sdk` for Polly, because the SDK does not expose the HTTP version of a Polly response. Connect timeout 10 s on both clients, plus a 60 s operation timeout on Polly.

### Live tests and bench

```bash
# from node-webrtc-rust/
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... AWS_REGION=... cargo test -p node-webrtc-rust-vendor-aws --features live --test live_test -- --nocapture --test-threads=1
NWR_BENCH_RUNS=3 AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... AWS_REGION=... cargo test -p node-webrtc-rust-vendor-aws --features live --test live_bench -- --nocapture --test-threads=1
```

All three variables must be set; otherwise the tests skip before any SDK client is built (so `~/.aws` is never read). `AWS_LIVE_REQUIRED=1` turns a missing variable into a panic that names it (CI sets it, `.github/workflows/vendor-live-aws.yml`; secrets `AWS_LIVE_ACCESS_KEY_ID`, `AWS_LIVE_SECRET_ACCESS_KEY` and `AWS_LIVE_REGION`). The STT test synthesizes its phrase with Polly. The bench runs through `vendor-bench.yml`.
