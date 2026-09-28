# vendor-azure

Azure AI Speech STT/TTS for `@node-webrtc-rust/sdk` VoiceAgent (documented REST only).

## Docs

| | URL |
|---|---|
| STT short audio | [REST speech-to-text short](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short) |
| TTS REST | [REST text-to-speech](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech) |
| Language + voices (TTS tab) | [Language and voice support](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support?tabs=tts) |
| Regional voice list | [GET voices/list](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech#get-a-list-of-voices) |

## Environment

| Variable | Use |
|---|---|
| `AZURE_SPEECH_KEY` or `SPEECH_KEY` | Subscription key (`Ocp-Apim-Subscription-Key`) |
| `AZURE_SPEECH_REGION` | Regional TTS host (`{region}.tts.speech.microsoft.com`) |
| `AZURE_SPEECH_RESOURCE` | Resource hostname for STT (e.g. `myresource.cognitiveservices.azure.com`) |
| `AZURE_SPEECH_LANGUAGE` | Default BCP-47 language for STT (fallback `en-US`) |

`config.endpoint` can override STT host or TTS regional host. `config.apiKey` overrides speech keys.

## STT (`config.model` = recognition mode)

| Mode | Transport |
|---|---|
| `conversation` | REST short audio POST (default) |

**No partials:** Microsoft documents that the short-audio REST API returns **final results only**.

**Not implemented (by design):** Microsoft publishes a conversation **WebSocket URL** (`wss://…/stt/speech/recognition/conversation/…`) and Voice Live realtime JSON (`wss://…/voice-live/realtime`). Official “how to recognize speech” directs clients to the **Speech SDK**; the short-audio REST page does **not** publish the SDK binary/text frame protocol (`Path`, `X-RequestId`, `speech.hypothesis`, …) for custom raw clients. This crate does **not** reverse-engineer SDK frames. Live partial STT is a documented gap; use a vendor with published streaming (e.g. Deepgram, AWS Transcribe) if you need `user_speech_partial`.

## TTS (`config.voice`)

**761** official TTS ShortNames from the [language-support TTS table](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support?tabs=tts) (snapshot **2026-09-28** in `src/voices.rs` / `azure-tts-voices.txt`). Includes Neural, MultilingualNeural, DragonHD (`locale-Name:DragonHDLatestNeural`), MAI-Voice-2, regional zh-CN variants, etc.

Default: `en-US-JennyNeural`. Unknown ShortNames are rejected at validation.

For the live catalog available in your Azure region, call **GET** `https://{region}.tts.speech.microsoft.com/cognitiveservices/voices/list` ([REST TTS docs](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech#get-a-list-of-voices)).

REST SSML with `X-Microsoft-OutputFormat: riff-16khz-16bit-mono-pcm`. Full-body synthesis only (no documented text-stream REST without SDK frames).

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-azure --lib
```
