# vendor-azure

Azure AI Speech STT/TTS for `@node-webrtc-rust/sdk` VoiceAgent (documented REST only).

## Docs

| | URL |
|---|---|
| STT short audio | [REST speech-to-text short](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short) |
| TTS REST | [REST text-to-speech](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech) |

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

**Not implemented:** Azure Voice Live / Speech SDK WebSocket partials — no published raw frame protocol in this crate without the C++ SDK. Live partial STT is a documented gap; use a vendor with documented streaming (e.g. Deepgram, AWS Transcribe) if you need `user_speech_partial`.

## TTS (`config.voice`)

Sample documented neural voices (extend list in `matrix.rs` as you validate more):

- `en-US-JennyNeural`, `en-US-GuyNeural`, `en-US-AriaNeural`

REST SSML with `X-Microsoft-OutputFormat: riff-16khz-16bit-mono-pcm`. Full-body synthesis only (no documented text-stream REST without SDK frames).

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-azure --lib
```
