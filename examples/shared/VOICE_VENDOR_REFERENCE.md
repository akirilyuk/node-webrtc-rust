# Voice STT/TTS vendor reference

Official API documentation for every speech provider supported by `@node-webrtc-rust/sdk/voice`.

Canonical machine-readable list: [`voice-vendor-docs.ts`](./voice-vendor-docs.ts).  
Sherpa local model catalog (download scripts): [`sherpa-local-model-catalog.json`](./sherpa-local-model-catalog.json).

---

## Speech-to-text (STT)

| Provider            | SDK id         | Default model (examples)                        | Official docs                                                                                                                              |
| ------------------- | -------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| OpenAI              | `openai`       | `whisper-1`                                     | [Speech to text](https://platform.openai.com/docs/guides/speech-to-text)                                                                   |
| Deepgram            | `deepgram`     | `nova-2`                                        | [Live streaming audio](https://developers.deepgram.com/docs/live-streaming-audio) · [Models](https://developers.deepgram.com/docs/models)  |
| Google Cloud        | `google`       | `latest_long`                                   | [Speech-to-Text](https://cloud.google.com/speech-to-text/docs)                                                                             |
| AssemblyAI          | `assemblyai`   | `universal-streaming-english`                   | [Streaming STT](https://www.assemblyai.com/docs/speech-to-text/streaming)                                                                  |
| Sherpa-ONNX (local) | `local-sherpa` | `sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06` | [Sherpa-ONNX](https://k2-fsa.github.io/sherpa/onnx/) · [Pre-trained models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models) |
| Mock                | `mock`         | _(deterministic test harness)_                  | [`crates/vendor-mock`](../../crates/vendor-mock/)                                                                                          |

### Local Sherpa-ONNX — free on-device STT (recommended)

**`local-sherpa` is the supported free STT path** — open-weight Sherpa-ONNX models run in Rust on your server. We encourage this flow when you control the worker:

- **Privacy:** user microphone audio is **not** streamed to third-party STT APIs (OpenAI, Deepgram, Google, etc.); only your WebRTC path and local inference see the PCM.
- **Latency:** no cloud STT HTTP/WebSocket round-trip — partial and final transcripts come from in-process `OnlineRecognizer`.
- **Cost:** no STT API keys or per-minute billing after you download model weights.

Example: [`voice-agent-local-sherpa`](../voice-agent-local-sherpa/README.md). Use cloud STT vendors above when you need a language or feature not covered by the local catalog.

On-device STT uses **streaming Zipformer transducer** bundles (encoder + decoder + joiner + `tokens.txt`). Weights are **not** bundled in npm — download via the example scripts below.

| Language             | Lang id | npm download script                 | Sherpa bundle                                           |
| -------------------- | ------- | ----------------------------------- | ------------------------------------------------------- |
| English (default)    | `en`    | `download-stt` or `download-stt:en` | `…-en-kroko-2025-08-06`                                 |
| English (2023 legacy) | `en-legacy` | `download-stt:en-legacy`        | `…-en-2023-06-26`                                       |
| Spanish              | `es`    | `download-stt:es`                   | `…-es-kroko-2025-08-06`                                 |
| French               | `fr`    | `download-stt:fr`                   | `…-fr-kroko-2025-08-06`                                 |
| German               | `de`    | `download-stt:de`                   | `…-de-kroko-2025-08-06`                                 |
| Chinese (Mandarin)   | `zh`    | `download-stt:zh`                   | `…-zh-int8-2025-06-30`                                  |
| Japanese             | `ja`    | `download-stt:ja`                   | `…-ar_en_id_ja_ru_th_vi_zh-2025-02-10` (multilingual)   |
| Arabic               | `ar`    | `download-stt:ar`                   | same multilingual bundle — set `SHERPA_STT_LANGUAGE=ar` |
| Russian              | `ru`    | `download-stt:ru`                   | `…-small-ru-vosk-int8-2025-08-16`                       |
| Bengali (South Asia) | `bn`    | `download-stt:bn`                   | `…-bn-vosk-2026-02-09`                                  |
| Hindi                | `hi`    | `download-stt:hi`                   | _No streaming Zipformer in official releases yet_       |
| Portuguese           | `pt`    | `download-stt:pt`                   | _Not available for `local-sherpa` yet_                  |
| Italian              | `it`    | `download-stt:it`                   | _Not available for `local-sherpa` yet_                  |

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

**Hindi / Portuguese / Italian:** Sherpa publishes some languages only in non–Zipformer bundles (e.g. `sherpa-onnx-cohere-transcribe-14-lang-int8-2026-04-01`), which are not wired to `local-sherpa` yet. Use cloud STT ([`voice-agent-browser`](../voice-agent-browser/README.md)) or Bengali (`bn`) as a South Asian alternative.

---

## Text-to-speech (TTS)

| Provider            | SDK id         | Default model (examples)       | Official docs                                                                                                                                   |
| ------------------- | -------------- | ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| OpenAI              | `openai`       | `tts-1`                        | [Text to speech](https://platform.openai.com/docs/guides/text-to-speech)                                                                        |
| ElevenLabs          | `elevenlabs`   | `eleven_multilingual_v2`       | [TTS API](https://elevenlabs.io/docs/api-reference/text-to-speech/convert) · [Voices](https://elevenlabs.io/docs/voices)                        |
| Google Cloud        | `google`       | `en-US-Neural2-A`              | [Text-to-Speech](https://cloud.google.com/text-to-speech/docs) · [Voice list](https://cloud.google.com/text-to-speech/docs/voices)              |
| Cartesia            | `cartesia`     | `sonic-english`                | [TTS bytes API](https://docs.cartesia.ai/api-reference/tts/bytes) · [Models](https://docs.cartesia.ai/models)                                   |
| Sherpa-ONNX (local) | `local-sherpa` | `vits-piper-en_US-amy-low`     | [Sherpa TTS](https://k2-fsa.github.io/sherpa/onnx/tts/index.html) · [TTS models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/tts-models) |
| Mock                | `mock`         | _(deterministic test harness)_ | [`crates/vendor-mock`](../../crates/vendor-mock/)                                                                                               |

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
