# Voice STT/TTS vendor reference

Official API documentation for every speech provider supported by `@node-webrtc-rust/sdk/voice`.

Canonical machine-readable list: [`voice-vendor-docs.ts`](./voice-vendor-docs.ts).  
Sherpa local model catalog (download scripts): [`sherpa-local-model-catalog.json`](./sherpa-local-model-catalog.json).

---

## Speech-to-text (STT)

| Provider            | SDK id         | Default model (examples)                                     | Official docs                                                                                                                                                                    |
| ------------------- | -------------- | ------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OpenAI              | `openai`       | `whisper-1`                                                  | [Speech to text](https://platform.openai.com/docs/guides/speech-to-text)                                                                                                         |
| Deepgram            | `deepgram`     | `nova-2` / `nova-3` (listen); not `flux-*` on listen         | STT live WS `Results` + `interim_results` — [Live streaming](https://developers.deepgram.com/docs/live-streaming-audio) · **Flux listen** (`flux-general`, etc.) is **not** in the documented listen matrix |
| ElevenLabs          | `elevenlabs`   | `scribe_v2_realtime` (STT)                                   | [Realtime STT](https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime) (`partial_transcript`, `committed_transcript`)                             |
| Google Cloud        | `google`       | `latest_long` (REST); `chirp_3` / `chirp_2` (V2 streaming)   | V2 `StreamingRecognize` + `interim_results` — [Chirp 3](https://docs.cloud.google.com/speech-to-text/docs/models/chirp-3) · [Speech-to-Text](https://cloud.google.com/speech-to-text/docs) |
| AssemblyAI          | `assemblyai`   | `universal-streaming-english`                                | v3 WS `Turn` events — [Streaming STT](https://www.assemblyai.com/docs/speech-to-text/streaming)                                                                                  |
| Groq                | `groq`         | `whisper-large-v3-turbo`                                     | [Speech to text](https://console.groq.com/docs/speech-to-text) — **file multipart only** (no live socket in Groq docs)                                                           |
| Azure AI Speech     | `azure`        | `conversation` (REST short audio)                            | [REST short STT](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short) — **final `DisplayText` only** (no partials)                        |
| AWS                 | `aws`          | `en-US` (language code)                                      | [Transcribe streaming](https://docs.aws.amazon.com/transcribe/latest/dg/streaming-setting-up.html) — SDK `StartStreamTranscription` partials + finals                            |
| Sherpa-ONNX (local) | `local-sherpa` | `sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06`        | [Sherpa-ONNX](https://k2-fsa.github.io/sherpa/onnx/) · [Pre-trained models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models)                                       |
| Mock                | `mock`         | _(deterministic test harness)_                               | [`crates/vendor-mock`](../../crates/vendor-mock/)                                                                                                                                |

### Local Sherpa-ONNX — free on-device STT (recommended)

**`local-sherpa` is the supported free STT path** — open-weight Sherpa-ONNX models run in Rust on your server. We encourage this flow when you control the worker:

- **Privacy:** user microphone audio is **not** streamed to third-party STT APIs (OpenAI, Deepgram, Google, etc.); only your WebRTC path and local inference see the PCM.
- **Latency:** no cloud STT HTTP/WebSocket round-trip — partial and final transcripts come from in-process `OnlineRecognizer`.
- **Cost:** no STT API keys or per-minute billing after you download model weights.

Example: [`voice-agent-local-sherpa`](../voice-agent-local-sherpa/README.md). Use cloud STT vendors above when you need a language or feature not covered by the local catalog.

On-device STT uses **streaming Zipformer transducer** bundles (encoder + decoder + joiner + `tokens.txt`). Weights are **not** bundled in npm — download via the example scripts below.

| Language              | Lang id     | npm download script                 | Sherpa bundle                                           |
| --------------------- | ----------- | ----------------------------------- | ------------------------------------------------------- |
| English (default)     | `en`        | `download-stt` or `download-stt:en` | `…-en-kroko-2025-08-06`                                 |
| English (2023 legacy) | `en-legacy` | `download-stt:en-legacy`            | `…-en-2023-06-26`                                       |
| Spanish               | `es`        | `download-stt:es`                   | `…-es-kroko-2025-08-06`                                 |
| French                | `fr`        | `download-stt:fr`                   | `…-fr-kroko-2025-08-06`                                 |
| German                | `de`        | `download-stt:de`                   | `…-de-kroko-2025-08-06`                                 |
| Chinese (Mandarin)    | `zh`        | `download-stt:zh`                   | `…-zh-int8-2025-06-30`                                  |
| Japanese              | `ja`        | `download-stt:ja`                   | `…-ar_en_id_ja_ru_th_vi_zh-2025-02-10` (multilingual)   |
| Arabic                | `ar`        | `download-stt:ar`                   | same multilingual bundle — set `SHERPA_STT_LANGUAGE=ar` |
| Russian               | `ru`        | `download-stt:ru`                   | `…-small-ru-vosk-int8-2025-08-16`                       |
| Bengali (South Asia)  | `bn`        | `download-stt:bn`                   | `…-bn-vosk-2026-02-09`                                  |
| Hindi                 | `hi`        | `download-stt:hi`                   | _No streaming Zipformer in official releases yet_       |
| Portuguese            | `pt`        | `download-stt:pt`                   | _Not available for `local-sherpa` yet_                  |
| Italian               | `it`        | `download-stt:it`                   | _Not available for `local-sherpa` yet_                  |

List every entry (including unavailable ones with notes):

```bash
npm run download-stt:list --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Download and configure (example — German):

```bash
npm run download-stt:de --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
export SHERPA_STT_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06"
export SHERPA_STT_LANGUAGE=de   # optional — inferred from model path when omitted
```

| Env var                 | Required | Purpose                                                                 |
| ----------------------- | -------- | ----------------------------------------------------------------------- |
| `SHERPA_STT_MODEL_PATH` | **Yes**  | Directory with `tokens.txt` and encoder/decoder/joiner `.onnx` files    |
| `SHERPA_STT_LANGUAGE`   | No       | Sets `stt.language` (must match bundle; required for multilingual pack) |

**Hindi / Portuguese / Italian:** Sherpa publishes some languages only in non–Zipformer bundles (e.g. `sherpa-onnx-cohere-transcribe-14-lang-int8-2026-04-01`), which are not wired to `local-sherpa` yet. Use cloud STT ([`voice-agent-browser`](../voice-agent-browser/README.md)) or Bengali (`bn`) as a South Asian alternative. **Future:** offline Sherpa STT plan — [`docs/offline-sherpa-stt-plan.md`](../../docs/offline-sherpa-stt-plan.md).

---

## Text-to-speech (TTS)

| Provider            | SDK id         | Default model (examples)                                   | Official docs                                                                                                                                                                                                                      |
| ------------------- | -------------- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OpenAI              | `openai`       | `tts-1`                                                    | [Text to speech](https://platform.openai.com/docs/guides/text-to-speech) — chunked PCM or SSE (`speech.audio.delta`)                                                                                                               |
| Deepgram            | `deepgram`     | `aura-asteria-en` (Aura `/v1`); `flux-haley-en` (Flux `/v2`) | Aura REST + WS; Flux REST + WS (`SpeechStarted`, `SpeechMetadata`) — [Aura speak](https://developers.deepgram.com/reference/text-to-speech/speak) · [Flux TTS](https://developers.deepgram.com/docs/flux-tts/overview)          |
| ElevenLabs          | `elevenlabs`   | `eleven_multilingual_v2`                                   | HTTP `/stream` + WS `stream-input` (**not** `eleven_v3`) — [Stream API](https://elevenlabs.io/docs/api-reference/text-to-speech/stream) · [Realtime WS](https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts) |
| Google Cloud        | `google`       | `en-US-Neural2-A` (batch); `en-US-Chirp3-HD-*` (streaming) | REST `text:synthesize` or `StreamingSynthesize` (Chirp 3 HD only) — [Streaming TTS](https://docs.cloud.google.com/text-to-speech/docs/create-audio-text-streaming) · [Voices](https://cloud.google.com/text-to-speech/docs/voices) |
| Cartesia            | `cartesia`     | `sonic-3`                                                  | WS `chunk` / `done` + `/tts/bytes` fallback — [WebSocket](https://docs.cartesia.ai/api-reference/tts/websocket) · [Bytes](https://docs.cartesia.ai/api-reference/tts/bytes)                                                        |
| Groq                | `groq`         | `canopylabs/orpheus-v1-english`                            | [TTS](https://console.groq.com/docs/text-to-speech) — full-body WAV (`POST …/audio/speech`); no documented progressive stream                                                                                                      |
| Azure AI Speech     | `azure`        | `en-US-JennyNeural` (voice id)                             | [REST TTS](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech) — SSML full-body PCM                                                                                                           |
| AWS                 | `aws`          | `Joanna` (Polly neural voice)                              | Polly `SynthesizeSpeech` PCM — [API](https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html). **`StartSpeechSynthesisStream`** (generative) **not** implemented                                                     |
| Sherpa-ONNX (local) | `local-sherpa` | `vits-piper-en_US-amy-low`                                 | [Sherpa TTS](https://k2-fsa.github.io/sherpa/onnx/tts/index.html) · [TTS models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/tts-models)                                                                                    |
| Mock                | `mock`         | _(deterministic test harness)_                             | [`crates/vendor-mock`](../../crates/vendor-mock/)                                                                                                                                                                                  |

---

## Streaming transport matrix (VoiceAgent)

How each vendor maps to **`user_speech_partial`** and progressive TTS playback. Matrices live in `crates/vendor-*/src/matrix.rs` (and `tts_matrix.rs` for Deepgram).

### STT — partials vs finalize-only

| Provider   | Transport (documented)                         | `user_speech_partial` | Notes |
| ---------- | ---------------------------------------------- | --------------------- | ----- |
| `openai`   | File JSON / file SSE / Realtime WS             | Model-dependent       | File models: partials only on SSE or Realtime; `whisper-1` is finalize-only file JSON |
| `deepgram` | `wss://…/v1/listen` (`nova-2`, `nova-3`)     | Yes (`interim_results`) | **Flux listen** models are **not** wired — use `nova-*` only |
| `elevenlabs` | Scribe v2 Realtime WS                        | Yes                   | `partial_transcript` / `committed_transcript` |
| `google`   | V2 `StreamingRecognize` or V1 REST           | Chirp streaming yes; `latest_long` REST finalize-only | V2 recognizer path required for Chirp streaming |
| `assemblyai` | v3 streaming WS                              | Yes (`Turn`)          | STT-only vendor |
| `groq`     | Multipart `POST …/transcriptions` on VAD end | **No**                | No Groq live listen socket in public docs |
| `azure`    | REST short audio                             | **No**                | Final `DisplayText` only; Voice Live / SDK WS not in this crate |
| `aws`      | Transcribe streaming (AWS SDK)               | Yes                   | Language code as `model` |
| `local-sherpa` | In-process Zipformer                     | Yes                   | No network |
| `mock`     | In-process                                   | Optional              | Test harness |

### TTS — progressive vs full-body

| Provider   | Default progressive path              | Full-body fallback | Notes |
| ---------- | ------------------------------------- | ------------------ | ----- |
| `openai`   | Chunked PCM (`stream_format=audio`) | Yes                | SSE (`speech.audio.delta`) for supported models |
| `deepgram` | Aura/Flux REST or WS speak            | Yes                | Route `aura-*` → `/v1/speak`, `flux-*` → `/v2/speak` |
| `elevenlabs` | HTTP `/stream`                      | Yes (`eleven_v3`)  | WS `stream-input` implemented; not for `eleven_v3` |
| `google`   | `StreamingSynthesize` (Chirp 3 HD)  | `text:synthesize` for Neural2 | Progressive only on documented streaming voices |
| `cartesia` | WebSocket contexts                  | `/tts/bytes`       | |
| `groq`     | —                                     | WAV response       | Drains full response then frames for outbound track |
| `azure`    | —                                     | REST SSML          | |
| `aws`      | —                                     | Polly `SynthesizeSpeech` | No Polly generative stream |
| `local-sherpa` | In-process Piper/VITS             | —                  | |
| `mock`     | —                                     | Instant test audio | |

### Dual-role vendors (STT + TTS in one SDK id)

| SDK id       | Example pairing (single API key where possible) |
| ------------ | ----------------------------------------------- |
| `deepgram`   | `nova-2` listen + `aura-asteria-en` speak       |
| `elevenlabs` | `scribe_v2_realtime` + `eleven_multilingual_v2` |
| `openai`     | `whisper-1` / `gpt-4o-mini-transcribe` + `tts-1` |
| `google`     | Chirp / `latest_long` + Neural2 or Chirp3 HD TTS |
| `groq`       | `whisper-large-v3-turbo` + Orpheus TTS          |
| `azure`      | REST STT + REST TTS (same speech key/region)    |
| `aws`        | Transcribe streaming + Polly neural               |

---

## Product home pages

| Provider            | Home                                    |
| ------------------- | --------------------------------------- |
| OpenAI              | https://platform.openai.com/docs        |
| Deepgram            | https://developers.deepgram.com/        |
| ElevenLabs          | https://elevenlabs.io/docs              |
| Cartesia            | https://docs.cartesia.ai/               |
| AssemblyAI          | https://www.assemblyai.com/docs         |
| Google Cloud Speech | https://cloud.google.com/speech-to-text |
| Google Cloud TTS    | https://cloud.google.com/text-to-speech |
| Groq                | https://console.groq.com/docs           |
| Azure AI Speech     | https://learn.microsoft.com/azure/ai-services/speech-service/ |
| AWS Transcribe/Polly | https://docs.aws.amazon.com/transcribe/ |
| Sherpa-ONNX         | https://github.com/k2-fsa/sherpa-onnx   |

---

## Where this is used in the repo

| Location                                                                      | Purpose                                          |
| ----------------------------------------------------------------------------- | ------------------------------------------------ |
| [`voice-vendor-presets.ts`](./voice-vendor-presets.ts)                        | Live cloud demo configs                          |
| [`sherpa-local-model-catalog.json`](./sherpa-local-model-catalog.json)        | Pinned Sherpa bundles + download script metadata |
| [`voice-agent/README.md`](../voice-agent/README.md)                           | Node loopback live demos                         |
| [`voice-agent-browser/README.md`](../voice-agent-browser/README.md)           | Browser + cloud vendors                          |
| [`voice-agent-local-sherpa/README.md`](../voice-agent-local-sherpa/README.md) | Browser + local Sherpa + per-language downloads  |
| [`packages/sdk/README.md`](../../packages/sdk/README.md)                      | SDK voice API                                    |

When adding a vendor, update **`voice-vendor-docs.ts`** and this file together.  
When adding a Sherpa STT language, update **`sherpa-local-model-catalog.json`**, the example `package.json` scripts, and this file.  
When adding a Sherpa TTS voice, update **`sherpa-tts-model-catalog.json`**, `download-tts:*` scripts, and this file.
