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

## STT (`config.model` = language code)

Documented streaming language codes include `en-US` (default), `es-US`, `fr-FR`, `de-DE`, `pt-BR`, `ja-JP`, `ko-KR`, `zh-CN`, `it-IT`, `hi-IN`.

Transport: **Amazon Transcribe streaming** via AWS SDK (`StartStreamTranscription`, PCM 16 kHz). Partial and final results from documented `TranscriptEvent` JSON.

## TTS (`config.voice`)

Sample Polly neural voices: `Joanna`, `Matthew`, `Amy`, `Brian`, `Ruth`, `Stephen`.

Transport: **Polly `SynthesizeSpeech`** PCM 16 kHz (`Engine::Neural`). `StartSpeechSynthesisStream` (generative-only) is **not** implemented in this crate.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-aws --lib
```

Live tests require AWS credentials and are skipped in CI when unset.
