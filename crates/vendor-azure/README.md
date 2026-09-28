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

## Streaming decisions (STT)

| Choice | What we implemented | Why |
|---|---|---|
| Transport | REST short audio `POST …/stt/speech/recognition/conversation/…` | Official [short-audio REST](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short). **Final `DisplayText` only** — no interims. |
| Mode | `conversation` only | Documented REST path segment. `dictation` / `interactive` are not wired. |
| Timing | Buffer PCM until `finalize_utterance`, then one POST | Matches the short-audio API (utterance file, not live frames). |

**Not implemented (by design):**

1. **Speech SDK WebSocket** (`wss://…/stt/speech/recognition/conversation/…`). Microsoft publishes the **URL** (custom-speech endpoint links, format notes). Official “how to recognize speech” tells you to use the **Speech SDK**. The short-audio REST page does **not** specify `Path` / `X-RequestId` / `speech.hypothesis` framing for a raw client. We do not reverse-engineer SDK frames.

2. **Voice Live** (`wss://…/voice-live/realtime?api-version=…&model=…`). Documented JSON events, compatible with Azure OpenAI Realtime. It is a **full realtime conversation** (instructions, Azure VAD, model replies, output voice) — not a drop-in `SttProvider` for VoiceAgent. VoiceAgent already owns VAD, the customer agent LLM (`@voicethere/agent` / local handler), and TTS. Voice Live would replace that stack (like a hosted realtime agent), not fill Azure REST partials. OpenAI in this repo uses Realtime as **`session.type=transcription` only**; Voice Live is not documented as that same transcription-only VoiceAgent adapter.

If you need `user_speech_partial`, use Deepgram listen, AWS Transcribe, ElevenLabs Scribe, Google Chirp V2, or OpenAI `gpt-live-transcribe`.

## TTS (`config.voice`)

**761** official TTS ShortNames from the [language-support TTS table](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support?tabs=tts) (snapshot **2026-09-28** in `src/voices.rs` / `azure-tts-voices.txt`). Includes Neural, MultilingualNeural, DragonHD (`locale-Name:DragonHDLatestNeural`), MAI-Voice-2, regional zh-CN variants, etc.

Default: `en-US-JennyNeural`. Unknown ShortNames are rejected at validation.

For the live catalog available in your Azure region, call **GET** `https://{region}.tts.speech.microsoft.com/cognitiveservices/voices/list` ([REST TTS docs](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech#get-a-list-of-voices)).

REST SSML with `X-Microsoft-OutputFormat: riff-16khz-16bit-mono-pcm`. Full-body synthesis only (no documented text-stream REST without SDK frames).

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-azure --lib
```
