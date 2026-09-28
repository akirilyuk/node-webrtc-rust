# vendor-aws

AWS Transcribe streaming STT and Polly TTS for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

| | URL |
|---|---|
| Transcribe streaming | [Streaming setup](https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html) |
| Polly | [SynthesizeSpeech](https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html) |

## Environment

Standard AWS SDK credential chain:

| Variable | Use |
|---|---|
| `AWS_ACCESS_KEY_ID` | IAM access key |
| `AWS_SECRET_ACCESS_KEY` | Secret key |
| `AWS_REGION` | Region for Transcribe streaming and Polly |

Optional session token via `AWS_SESSION_TOKEN`.

## Streaming decisions (STT)

| Choice | What we implemented | Why |
|---|---|---|
| Transport | AWS SDK `StartStreamTranscription` (PCM 16 kHz) | Official [Transcribe streaming](https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html). |
| Partials | `TranscriptEvent` `IsPartial` + `Alternatives[0].Transcript` | Documented event JSON. |
| `config.model` | Language code (`en-US` default) | Streaming examples use language codes, not a separate model id. Allowlist in `src/matrix.rs`. |

## Streaming decisions (TTS)

| Choice | What we implemented | Why |
|---|---|---|
| Transport | Polly `SynthesizeSpeech`, `Engine::Neural`, PCM 16 kHz | Official [SynthesizeSpeech](https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html). Full-body then 20 ms VoiceAgent frames. |
| Voices | Sample neural ids: `Joanna` (default), `Matthew`, `Amy`, `Brian`, `Ruth`, `Stephen` | Documented neural voices. Unknown ids fail validation. |
| **Not** `StartSpeechSynthesisStream` | — | Official stream API exists but **only the `generative` engine** is supported. Neural voices on that call error. Progressive TTS for `Joanna` is **not** documented on `SynthesizeSpeech`. Generative stream is a second product (HTTP/2 `TextEvent` / `AudioEvent`), not an upgrade of the neural path. |

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-aws --lib
```

Live tests require AWS credentials and are skipped in CI when unset.
