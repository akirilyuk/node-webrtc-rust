# vendor-azure

Azure AI Speech STT/TTS for `@node-webrtc-rust/sdk` VoiceAgent (documented REST only).

## Docs

|                             | URL                                                                                                                            |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| STT short audio             | [REST speech-to-text short](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short)      |
| TTS REST                    | [REST text-to-speech](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech)                  |
| Language + voices (TTS tab) | [Language and voice support](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support?tabs=tts)     |
| Regional voice list         | [GET voices/list](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech#get-a-list-of-voices) |

## Environment

| Variable                           | Use                                                                       |
| ---------------------------------- | ------------------------------------------------------------------------- |
| `AZURE_SPEECH_KEY` or `SPEECH_KEY` | Subscription key (`Ocp-Apim-Subscription-Key`)                            |
| `AZURE_SPEECH_REGION`              | Regional TTS host (`{region}.tts.speech.microsoft.com`)                   |
| `AZURE_SPEECH_RESOURCE`            | Resource hostname for STT (e.g. `myresource.cognitiveservices.azure.com`) |
| `AZURE_SPEECH_LANGUAGE`            | Default BCP-47 language for STT (fallback `en-US`)                        |

`config.endpoint` can override STT host or TTS regional host. `config.apiKey` overrides speech keys.

## Streaming decisions (STT)

| Choice    | What we implemented                                             | Why                                                                                                                                                                    |
| --------- | --------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Transport | REST short audio `POST …/stt/speech/recognition/conversation/…` | Official [short-audio REST](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-speech-to-text-short). **Final `DisplayText` only** — no interims. |
| Mode      | `conversation` only                                             | Documented REST path segment. `dictation` / `interactive` are not wired.                                                                                               |
| Timing    | Buffer PCM until `finalize_utterance`, then one POST            | Matches the short-audio API (utterance file, not live frames).                                                                                                         |

**Not implemented (by design):**

1. **Speech SDK WebSocket** (`wss://…/stt/speech/recognition/conversation/…`). Microsoft publishes the **URL** (custom-speech endpoint links, format notes). Official “how to recognize speech” tells you to use the **Speech SDK**. The short-audio REST page does **not** specify `Path` / `X-RequestId` / `speech.hypothesis` framing for a raw client. We do not reverse-engineer SDK frames.

2. **Voice Live** (`wss://…/voice-live/realtime?api-version=…&model=…`). Documented JSON events, compatible with Azure OpenAI Realtime. It is a **full realtime conversation** (instructions, Azure VAD, model replies, output voice) — not a drop-in `SttProvider` for VoiceAgent. VoiceAgent already owns VAD, the customer agent LLM (`@voicethere/agent` / local handler), and TTS. Voice Live would replace that stack (like a hosted realtime agent), not fill Azure REST partials. OpenAI in this repo uses Realtime as **`session.type=transcription` only**; Voice Live is not documented as that same transcription-only VoiceAgent adapter.

If you need `user_speech_partial`, use Deepgram listen, AWS Transcribe, ElevenLabs Scribe, Google Chirp V2, or OpenAI `gpt-live-transcribe`.

## TTS (`config.voice`)

**761** official TTS ShortNames from the [language-support TTS table](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/language-support?tabs=tts) (snapshot **2026-09-28** in `packages/voice-catalog/catalog/azure-tts-voices.json`). Includes Neural, MultilingualNeural, DragonHD (`locale-Name:DragonHDLatestNeural`), MAI-Voice-2, regional zh-CN variants, etc.

Default: `en-US-JennyNeural`. Unknown ShortNames are rejected at validation.

For the live catalog available in your Azure region, call **GET** `https://{region}.tts.speech.microsoft.com/cognitiveservices/voices/list` ([REST TTS docs](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech#get-a-list-of-voices)).

REST SSML with `X-Microsoft-OutputFormat: raw-48khz-16bit-mono-pcm`, read as it arrives (see Transport). No documented text-stream REST without SDK frames.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-azure --lib
cargo test -p node-webrtc-rust-vendor-azure --features live --lib --test http_pool_test --test mock_server_test --test vendor_test
```

The live checks against the real API (`tests/live_test.rs`) and the latency bench (`tests/live_bench.rs`) are described under Transport.

## Transport

Config: `tts.transport` and `stt.transport`: `auto` (default) or `http`. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

Both legs are REST requests (the realtime speech API is SDK-only).

**TTS streams the raw PCM body.** `raw-48khz-16bit-mono-pcm` is listed under "Streaming" audio outputs, and the REST page says of a `200` answer: "This file can be played as it's transferred, saved to a buffer, or saved to a file." (The `riff-*` formats are listed as non-streaming.) 48 kHz is the agent's rate, so there is no resampling, and it is a higher rate than the 16 kHz WAV used before ("If you select 48kHz output format, the high-fidelity voice model with 48kHz will be invoked"). The body goes to the sink as it arrives, each chunk once, an odd trailing byte carried to the next chunk. A body that errors or ends early after audio is an error with no retry, and a body without audio is an error (`Azure TTS returned no audio`). Bounds: first audio 15 s (request and headers included), 10 s between chunks, 60 s for the whole request. The request also sends the `User-Agent` header the page marks as required.

**STT is a finals-only batch upload.** The short-audio page says "The REST API for short audio returns only final results. It doesn't provide partial results.", so `emits_partials()` is `false`. A failed request (network error, non-2xx, timeout, an answer that is not the documented JSON, `RecognitionStatus: Error`) is an `Err` from `poll_transcript`; the STT actor turns it into a vendor error event for the utterance. The status and Azure's error text are in the message, the key never is. `NoMatch`, `InitialSilenceTimeout` and `BabbleTimeout` are documented outcomes for audio with nothing to transcribe: no final and no error. Each finalize is an independent request, so the bench measures a second utterance on the same provider as `warm` (pooled connection).

`auto` resolves to the catalog default, which is today's behaviour: TTS `http`, STT `http`. Default measured in a later change.

HTTP requests use one pooled client per provider; HTTP/2 when the vendor negotiates it (ALPN), otherwise HTTP/1.1. Connect timeout 10 s, whole-request timeout 60 s, idle pool 90 s, TCP keepalive 30 s.

### Live tests and bench

```bash
# from node-webrtc-rust/
AZURE_SPEECH_KEY=... AZURE_SPEECH_REGION=... cargo test -p node-webrtc-rust-vendor-azure --features live --test live_test -- --nocapture --test-threads=1
NWR_BENCH_RUNS=3 AZURE_SPEECH_KEY=... AZURE_SPEECH_REGION=... cargo test -p node-webrtc-rust-vendor-azure --features live --test live_bench -- --nocapture --test-threads=1
```

Without both variables they skip; `AZURE_LIVE_REQUIRED=1` turns a missing one into a panic that names it (CI sets it, `.github/workflows/vendor-live-azure.yml`; secrets `AZURE_LIVE_SPEECH_KEY` and `AZURE_LIVE_SPEECH_REGION`). The STT test synthesizes its phrase with Azure TTS. The bench runs through `vendor-bench.yml`. The mock-server tests run in PR integration (`cargo vendor-azure`).

## Latency benchmark

The TTS and STT latency of this crate against the real API is measured with
`npm run bench:vendors -- --vendors azure` (from `node-webrtc-rust/`, keys as in the
[benchmark quick start](../../benchmarks/README.md)). What each number means:
[`benchmarks/METHODOLOGY.md`](../../benchmarks/METHODOLOGY.md).
