# vendor-mistral

Mistral Voxtral TTS for `@node-webrtc-rust/sdk` VoiceAgent. Default model: `voxtral-mini-tts-3-0`. Realtime STT is added in a later change.

## Docs

|                 | URL                                                                                    |
| --------------- | -------------------------------------------------------------------------------------- |
| Generate speech | [generate-speech](https://docs.mistral.ai/studio/audio/text-to-speech/generate-speech) |
| Stream speech   | [stream-speech](https://docs.mistral.ai/studio/audio/text-to-speech/stream-speech)     |
| Voices          | [voices](https://docs.mistral.ai/studio/audio/text-to-speech/voices)                   |

## Environment

| Variable          | Use                                                          |
| ----------------- | ------------------------------------------------------------ |
| `MISTRAL_API_KEY` | API key (`Authorization: Bearer`) when `tts.apiKey` is unset |

`tts.endpoint` replaces the API origin (default `https://api.mistral.ai`).

## TTS path

| Choice        | What we implemented                                                                                                | Why                                                                                                                                 |
| ------------- | ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Transport     | One: `POST /v1/audio/speech` with `stream: true`, answered as server-sent events. No retry, no fallback            | The only documented streaming TTS path. A retry after audio started would play the start of the utterance twice.                    |
| Request body  | `model`, `input`, `voice_id`, `response_format: "wav"`, `stream: true`. Nothing else                               | Only documented fields. `ref_audio`, `expressiveness`, `phoneme_hints` and the rest are not used.                                   |
| Events        | `speech.audio.delta` (`audio_data`, base64) and `speech.audio.done`. Unknown types are skipped                     | A body that ends before `speech.audio.done` is an error (`mistral tts stream ended before speech.audio.done`).                      |
| Audio format  | WAV, decoded from its header: tag 1 with 16 bits (s16le) or tag 3 with 32 bits (f32le), mono, rate from the header | The docs do not state the PCM sample format. Header bytes and other chunks are never played. The header may be split across deltas. |
| Resampling    | 24 kHz mono to 48 kHz stereo s16le with the speech crate's `MonoToStereo48k`                                       | Same helper as the Fish Audio vendor.                                                                                               |
| Model         | `voxtral-mini-tts-3-0` or `voxtral-mini-tts-3`, validated before anything is sent                                  | `voxtral-mini-tts-latest` does not point to v3 yet, so it is rejected, never mapped.                                                |
| Voice         | `tts.voice`, else `voice_478` (`""` and `"default"` count as unset)                                                | Mistral answers 400 without a voice.                                                                                                |
| Bounds        | connect 10 s, headers and first audio 15 s, idle between chunks 10 s, total 60 s                                   | Every wait has a named error (`mistral http first audio timed out after 15000 ms`).                                                 |
| Errors        | `mistral http <status>: type=<type> code=<code> <message>`; 422 lists `message.detail[].msg`                       | Request text is never logged and never appears in an error.                                                                         |
| Sink contract | Every PCM chunk goes to the sink once, as it arrives                                                               | The returned chunk is the whole utterance for the phrase cache and is never re-sent to the sink.                                    |

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-mistral --features live --lib --test tts_mock_server_test
```

The `live` feature is declared for the real-network tests that come with the live CI job.
