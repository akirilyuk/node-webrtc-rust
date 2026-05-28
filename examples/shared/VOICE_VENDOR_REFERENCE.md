# Voice STT/TTS vendor reference

Official API documentation for every speech provider supported by `@node-webrtc-rust/sdk/voice`.

Canonical machine-readable list: [`voice-vendor-docs.ts`](./voice-vendor-docs.ts).

---

## Speech-to-text (STT)

| Provider | SDK id | Default model (examples) | Official docs |
|----------|--------|--------------------------|---------------|
| OpenAI | `openai` | `whisper-1` | [Speech to text](https://platform.openai.com/docs/guides/speech-to-text) |
| Deepgram | `deepgram` | `nova-2` | [Live streaming audio](https://developers.deepgram.com/docs/live-streaming-audio) · [Models](https://developers.deepgram.com/docs/models) |
| Google Cloud | `google` | `latest_long` | [Speech-to-Text](https://cloud.google.com/speech-to-text/docs) |
| AssemblyAI | `assemblyai` | `universal-streaming-english` | [Streaming STT](https://www.assemblyai.com/docs/speech-to-text/streaming) |
| Sherpa-ONNX (local) | `local-sherpa` | `sherpa-onnx-streaming-zipformer-en-2023-06-26` | [Sherpa-ONNX](https://k2-fsa.github.io/sherpa/onnx/) · [Pre-trained models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models) |
| Mock | `mock` | _(deterministic test harness)_ | [`crates/vendor-mock`](../../crates/vendor-mock/) |

---

## Text-to-speech (TTS)

| Provider | SDK id | Default model (examples) | Official docs |
|----------|--------|--------------------------|---------------|
| OpenAI | `openai` | `tts-1` | [Text to speech](https://platform.openai.com/docs/guides/text-to-speech) |
| ElevenLabs | `elevenlabs` | `eleven_multilingual_v2` | [TTS API](https://elevenlabs.io/docs/api-reference/text-to-speech/convert) · [Voices](https://elevenlabs.io/docs/voices) |
| Google Cloud | `google` | `en-US-Neural2-A` | [Text-to-Speech](https://cloud.google.com/text-to-speech/docs) · [Voice list](https://cloud.google.com/text-to-speech/docs/voices) |
| Cartesia | `cartesia` | `sonic-english` | [TTS bytes API](https://docs.cartesia.ai/api-reference/tts/bytes) · [Models](https://docs.cartesia.ai/models) |
| Mock | `mock` | _(deterministic test harness)_ | [`crates/vendor-mock`](../../crates/vendor-mock/) |

---

## Product home pages

| Provider | Home |
|----------|------|
| OpenAI | https://platform.openai.com/docs |
| Deepgram | https://developers.deepgram.com/ |
| ElevenLabs | https://elevenlabs.io/docs |
| Cartesia | https://docs.cartesia.ai/ |
| AssemblyAI | https://www.assemblyai.com/docs |
| Google Cloud Speech | https://cloud.google.com/speech-to-text |
| Google Cloud TTS | https://cloud.google.com/text-to-speech |
| Sherpa-ONNX | https://github.com/k2-fsa/sherpa-onnx |

---

## Where this is used in the repo

| Location | Purpose |
|----------|---------|
| [`voice-vendor-presets.ts`](./voice-vendor-presets.ts) | Live cloud demo configs |
| [`voice-agent/README.md`](../voice-agent/README.md) | Node loopback live demos |
| [`voice-agent-browser/README.md`](../voice-agent-browser/README.md) | Browser + cloud vendors |
| [`voice-agent-local-sherpa/README.md`](../voice-agent-local-sherpa/README.md) | Browser + local Sherpa |
| [`packages/sdk/README.md`](../../packages/sdk/README.md) | SDK voice API |

When adding a vendor, update **`voice-vendor-docs.ts`** and this file together.
