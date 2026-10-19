# vendor-google

Google Cloud Speech-to-Text and Text-to-Speech for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|                | URL                                                                                                       |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| Chirp / V2 STT | [Chirp 3](https://docs.cloud.google.com/speech-to-text/docs/models/chirp-3)                               |
| V2 recognizers | [Recognizers](https://cloud.google.com/speech-to-text/v2/docs/recognizers)                                |
| Streaming TTS  | [Streaming text-to-speech](https://docs.cloud.google.com/text-to-speech/docs/create-audio-text-streaming) |
| Voices         | [TTS voices](https://cloud.google.com/text-to-speech/docs/voices)                                         |

## Credentials

Google has two credential paths. Which one you have decides which models and voices work.

| Credential                                              | Unlocks                                                                                       |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| API key (`config.apiKey` or `GOOGLE_API_KEY`)           | REST only: STT `latest_long` (V1 `speech:recognize`), TTS `text:synthesize` (e.g. Neural2)    |
| Service account file (`GOOGLE_APPLICATION_CREDENTIALS`) | everything: REST and gRPC (STT `chirp_3` / `chirp_2` / `telephony`, Chirp 3 HD streaming TTS) |

The gRPC paths (V2 `StreamingRecognize`, `StreamingSynthesize`) need OAuth. None of the Google pages
read for this crate (authentication pages, the streaming guides, the REST references, which list
only the OAuth scope `https://www.googleapis.com/auth/cloud-platform`) document API-key auth for
those methods, so a key alone fails with a named config error, for example
`google stt chirp_3 needs a service account (GOOGLE_APPLICATION_CREDENTIALS) — API keys work only
for latest_long (REST)`. Application Default Credentials outside `GOOGLE_APPLICATION_CREDENTIALS`
(gcloud user credentials, the metadata server) are never read.

REST precedence, first match wins: explicit `config.apiKey`, `GOOGLE_API_KEY`, `GOOGLE_ACCESS_TOKEN`
(bearer), the service account file (bearer). Before this change `config.apiKey` was ignored and
only the environment was read. gRPC: the service account file only (`GOOGLE_ACCESS_TOKEN` is not
usable there).

The API-key form (`?key=`) on the two REST methods is not shown on the pages read for this
crate; the live API-key tests are the check that Google accepts it. The key is never logged and is
stripped from request errors.

## Recognizer (V2 STT)

Recognizers page: "Recognizers are optional in recognition requests. To make a request without a
recognizer, use the recognizer resource ID `_` in the location you are making a request." The model
and language go in the inline config of the first request. So by default the recognizer is
`projects/$GOOGLE_CLOUD_PROJECT/locations/$GOOGLE_SPEECH_LOCATION/recognizers/_`:

- `GOOGLE_CLOUD_PROJECT`: required for V2 models (a missing project fails at construction, naming it).
- `GOOGLE_SPEECH_LOCATION`: default `us`. Chirp 3 page: "Chirp 3 is available in the following Google
  Cloud regions, with more planned: `us (multi-region)` GA, `eu (multi-region)` GA"; `global` is not
  one of them. The gRPC endpoint follows the location (`https://us-speech.googleapis.com`,
  `https://eu-speech.googleapis.com`; `global` uses the default endpoint).
- `GOOGLE_SPEECH_RECOGNIZER`: default `_`.
- `stt.endpoint` = `projects/{project}/locations/{location}/recognizers/{recognizer}` overrides all three.

## Audio

| Path                   | What Google returns                                                      | What the sink gets                                                            |
| ---------------------- | ------------------------------------------------------------------------ | ----------------------------------------------------------------------------- |
| REST `text:synthesize` | JSON, base64 LINEAR16 **with a WAV header**, asked at 48000 Hz           | header stripped, rate from the header, 48 kHz stereo, in 200 ms chunks        |
| `StreamingSynthesize`  | "headerless LINEAR16 audio with a sample rate of 24000" (no rate is set) | 24 kHz upsampled with `MonoToStereo48k`, 48 kHz stereo, one chunk per message |

REST TTS is a JSON body with base64 audio: batch by nature. Nothing is readable before the whole
synthesis is done, so there is no progressive read; the finished audio is cut into chunks for the
sink. Only `StreamingSynthesize` (Chirp 3 HD voices) streams while synthesizing.

STT input is 16 kHz mono LINEAR16 on both paths.

## Bounds

| Wait                                             | Bound                                                                                                 |
| ------------------------------------------------ | ----------------------------------------------------------------------------------------------------- |
| REST connect / whole request                     | 10 s / 60 s                                                                                           |
| gRPC client connect (both clients)               | 10 s                                                                                                  |
| `StreamingSynthesize` request accepted           | 10 s per request                                                                                      |
| `StreamingSynthesize` first audio / idle / total | 15 s / 10 s / 60 s, named errors (`no audio within …`, `audio stalled for …`, `synthesis exceeded …`) |
| V2 stream open (config message)                  | 10 s                                                                                                  |
| V2 audio message send                            | 10 s                                                                                                  |
| V2 finalize                                      | `finalize_wait_ms()` (10 s), `no final within … of the half-close`                                    |

V2 audio goes out in pieces of at most 24000 bytes (guide: "There is a 25 KB limit on audio sent in the requests of a stream").
A failed V1 request or V2 stream is reported by `poll_transcript` / finalize, not swallowed.

## Streaming decisions (STT)

| Model                             | Transport                                   | `user_speech_partial` | Why                                                           |
| --------------------------------- | ------------------------------------------- | --------------------- | ------------------------------------------------------------- |
| `chirp_3`, `chirp_2`, `telephony` | V2 `StreamingRecognize` + `interim_results` | Yes                   | Official Chirp / V2 streaming. Requires a V2 recognizer path. |
| `latest_long`                     | V1 REST `speech:recognize` on finalize      | No                    | Documented batch/long-form REST. Not a streaming recognizer.  |

V2 uses one stream per utterance: the stream opens with the first audio, and the finalize half-closes
the request side so the server flushes the final results and ends the stream. The next utterance opens a new stream on the same gRPC client.

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
cargo test -p node-webrtc-rust-vendor-google --features live
```

Offline: the REST TTS WAV/rate checks (`tests/mock_server_test.rs`), the 24 kHz stream conversion and
all gRPC bounds (`collect_stream`, `finalize_stream` unit tests), recognizer resolution. The gRPC
calls themselves go to Google over TLS and cannot be faked by the local HTTP mock.

## Live tests and bench

`tests/live_test.rs` has one set per credential path:

| Set             | Variables                                                | Flag                        | Tests                                                                                   |
| --------------- | -------------------------------------------------------- | --------------------------- | --------------------------------------------------------------------------------------- |
| API key         | `GOOGLE_API_KEY`                                         | `GOOGLE_LIVE_REQUIRED=1`    | `google_tts_live_rest_neural2`, `google_stt_live_rest_latest_long_vad_gated_roundtrip`  |
| Service account | `GOOGLE_APPLICATION_CREDENTIALS`, `GOOGLE_CLOUD_PROJECT` | `GOOGLE_SA_LIVE_REQUIRED=1` | `google_tts_live_streaming_chirp3_hd`, `google_stt_live_v2_chirp_3_vad_gated_roundtrip` |

```bash
# from node-webrtc-rust/
GOOGLE_API_KEY=... GOOGLE_APPLICATION_CREDENTIALS=/path/sa.json GOOGLE_CLOUD_PROJECT=... \
  cargo test -p node-webrtc-rust-vendor-google --features live --test live_test -- --nocapture --test-threads=1
```

The service account tests pass no API key and assert the `grpc` transport. CI: `.github/workflows/vendor-live-google.yml`
(secrets `GOOGLE_LIVE_API_KEY`, `GOOGLE_LIVE_PROJECT_ID`, `GOOGLE_LIVE_SERVICE_ACCOUNT_JSON`) and the `google` option of
`.github/workflows/vendor-bench.yml` (`tests/live_bench.rs`: REST TTS and STT cold and warm; gRPC TTS and V2 STT cold).

## Transport

Config: `tts.transport` and `stt.transport`: `auto` only. The voice (TTS) or the model (STT) decides between REST and gRPC. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

An explicit `websocket` or `http` fails at construction with `google <leg> transport is decided by the model; use auto`.

`auto` resolves to the catalog default, which is today's behaviour: none (the voice or model decides). Default measured in a later change.

HTTP requests use one pooled client per provider; HTTP/2 when the vendor negotiates it (ALPN), otherwise HTTP/1.1. Connect timeout 10 s, whole-request timeout 60 s, idle pool 90 s, TCP keepalive 30 s. The gRPC paths (Chirp3-HD `StreamingSynthesize`, V2 `StreamingRecognize`) keep one gRPC client per provider and report `grpc`.
