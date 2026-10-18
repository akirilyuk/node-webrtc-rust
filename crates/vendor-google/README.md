# vendor-google

Google Cloud Speech-to-Text and Text-to-Speech for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|                | URL                                                                                                       |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| Chirp / V2 STT | [Chirp 3](https://docs.cloud.google.com/speech-to-text/docs/models/chirp-3)                               |
| V2 recognizers | [Recognizers](https://cloud.google.com/speech-to-text/v2/docs/recognizers)                                |
| Streaming TTS  | [Streaming text-to-speech](https://docs.cloud.google.com/text-to-speech/docs/create-audio-text-streaming) |
| Voices         | [TTS voices](https://cloud.google.com/text-to-speech/docs/voices)                                         |

## Environment

ADC or API key via `config.apiKey`. V2 streaming also needs a recognizer:

- `stt.endpoint` = `projects/{project}/locations/{location}/recognizers/{recognizer}`, or
- `GOOGLE_CLOUD_PROJECT` + `GOOGLE_SPEECH_LOCATION` + `GOOGLE_SPEECH_RECOGNIZER`

## Streaming decisions (STT)

| Model                             | Transport                                   | `user_speech_partial` | Why                                                           |
| --------------------------------- | ------------------------------------------- | --------------------- | ------------------------------------------------------------- |
| `chirp_3`, `chirp_2`, `telephony` | V2 `StreamingRecognize` + `interim_results` | Yes                   | Official Chirp / V2 streaming. Requires a V2 recognizer path. |
| `latest_long`                     | V1 REST `speech:recognize` on finalize      | No                    | Documented batch/long-form REST. Not a streaming recognizer.  |

Unknown model ids fail config (`DOCUMENTED_STT_MODELS` in `src/matrix.rs`).

We do **not** call V2 streaming for `latest_long` or invent a streaming path for Neural2-era V1-only models.

## Streaming decisions (TTS)

| Voice                                 | Transport                        | Why                                                        |
| ------------------------------------- | -------------------------------- | ---------------------------------------------------------- |
| Name contains `Chirp3-HD`             | `StreamingSynthesize`            | Official streaming TTS is **Chirp 3 HD only**.             |
| Other voices (e.g. `en-US-Neural2-A`) | REST `text:synthesize` full body | Same docs: Neural2 is not on the streaming TTS quickstart. |

Default STT model: `latest_long` (batch). Default TTS voice in examples: `en-US-Neural2-A` (batch). Use a Chirp3-HD voice when you want progressive TTS.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-google --lib
```

## Transport

Config: `tts.transport` and `stt.transport`: `auto` only. The voice (TTS) or the model (STT) decides between REST and gRPC. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

An explicit `websocket` or `http` fails at construction with `google <leg> transport is decided by the model; use auto`.

`auto` resolves to the catalog default, which is today's behaviour: none (the voice or model decides). Default measured in a later change.

HTTP requests use one pooled client per provider; HTTP/2 when the vendor negotiates it (ALPN), otherwise HTTP/1.1. Connect timeout 10 s, whole-request timeout 60 s, idle pool 90 s, TCP keepalive 30 s. The gRPC paths (Chirp3-HD `StreamingSynthesize`, V2 `StreamingRecognize`) keep one gRPC client per provider and report `grpc`.
