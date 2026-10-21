# vendor-mistral

Mistral Voxtral TTS and realtime STT for `@node-webrtc-rust/sdk` VoiceAgent. Default TTS model: `voxtral-mini-tts-realtime-3-0` (Voxtral TTS Realtime 3). Default STT model: `voxtral-mini-transcribe-realtime-3-0`.

## Docs

|                 | URL                                                                                    |
| --------------- | -------------------------------------------------------------------------------------- |
| Generate speech | [generate-speech](https://docs.mistral.ai/studio/audio/text-to-speech/generate-speech) |
| Stream speech   | [stream-speech](https://docs.mistral.ai/studio/audio/text-to-speech/stream-speech)     |
| Voices          | [voices](https://docs.mistral.ai/studio/audio/text-to-speech/voices)                   |

## Environment

| Variable           | Use                                                          |
| ------------------ | ------------------------------------------------------------ |
| `MISTRAL_API_KEY`  | API key (`Authorization: Bearer`) when `tts.apiKey` is unset |
| `MISTRAL_BASE_URL` | API origin for TTS and STT when the config sets no endpoint  |

Base URL for both TTS and STT: the config `endpoint` if set, else `MISTRAL_BASE_URL` if non-empty, else the global endpoint `https://api.mistral.ai`. Documented endpoints ([regional inference](https://docs.mistral.ai/studio/regional-inference)): global `https://api.mistral.ai`, EU `https://api.eu.mistral.ai`, US `https://api.us.mistral.ai`. A regional endpoint serves only the models hosted there.

## TTS path

| Choice        | What we implemented                                                                                                                                                                                                                                | Why                                                                                                                                                                                                                           |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Transport     | One: `POST /v1/audio/speech` with `stream: true`, answered as server-sent events, for the realtime and the other models alike. No retry, no fallback. There is no text-streaming socket (that docs section is empty), so text is sent per sentence | The only documented streaming TTS path. A retry after audio started would play the start of the utterance twice.                                                                                                              |
| Request body  | `model`, `input`, `voice_id`, `response_format: "wav"`, `stream: true`. Nothing else                                                                                                                                                               | Only documented fields. `ref_audio`, `expressiveness`, `phoneme_hints` and the rest are not used.                                                                                                                             |
| Events        | `speech.audio.delta` (`audio_data`, base64) and `speech.audio.done`. Unknown types are skipped                                                                                                                                                     | A body that ends before `speech.audio.done` is an error (`mistral tts stream ended before speech.audio.done`).                                                                                                                |
| Audio format  | WAV, decoded from its header: tag 1 with 16 bits (s16le) or tag 3 with 32 bits (f32le), mono, rate from the header                                                                                                                                 | The docs do not state the PCM sample format. Header bytes and other chunks are never played. The header may be split across deltas.                                                                                           |
| Resampling    | 24 kHz mono to 48 kHz stereo s16le with the speech crate's `MonoToStereo48k`                                                                                                                                                                       | Same helper as the Fish Audio vendor.                                                                                                                                                                                         |
| Model         | `voxtral-mini-tts-realtime-3-0` (default), `voxtral-mini-tts-3-0` or `voxtral-mini-tts-3`, validated before anything is sent                                                                                                                       | The realtime model streams with about 90 ms to first audio ([model page](https://docs.mistral.ai/models/voxtral-mini-tts-realtime-3-0)). `voxtral-mini-tts-latest` does not point to v3 yet, so it is rejected, never mapped. |
| Voice         | `tts.voice`, else `voice_478` (`""` and `"default"` count as unset)                                                                                                                                                                                | Mistral answers 400 without a voice.                                                                                                                                                                                          |
| Bounds        | connect 10 s, headers and first audio 15 s, idle between chunks 10 s, total 60 s                                                                                                                                                                   | Every wait has a named error (`mistral http first audio timed out after 15000 ms`).                                                                                                                                           |
| Errors        | `mistral http <status>: type=<type> code=<code> <message>`; 422 lists `message.detail[].msg`                                                                                                                                                       | Request text is never logged and never appears in an error.                                                                                                                                                                   |
| Sink contract | Every PCM chunk goes to the sink once, as it arrives                                                                                                                                                                                               | The returned chunk is the whole utterance for the phrase cache and is never re-sent to the sink.                                                                                                                              |

## Realtime STT path

Docs: [transcribe-in-realtime](https://docs.mistral.ai/studio/audio/speech-to-text/transcribe-in-realtime). Models: `voxtral-mini-transcribe-realtime-3-0` (default) and `voxtral-mini-transcribe-realtime-3`. `stt.endpoint` replaces the origin (`https` maps to `wss`, `http` to `ws`).

| Choice          | What we implemented                                                                                                                                           | Why                                                                                                                         |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| Session         | `wss://api.mistral.ai/v1/audio/transcriptions/realtime?model=<id>`, `Authorization: Bearer`. One session per utterance                                        | `input_audio.end` then `transcription.done` is the only documented final signal.                                            |
| Settings        | One `session.update` right after connect: `audio_format` `pcm_s16le` at 16000 Hz, `target_streaming_delay_ms: 240`, `language` only for a 2-letter code       | A later update is an error. `en-US`, `multi` and the like are left out so Mistral detects the language.                     |
| Audio           | 16 kHz mono s16le, batched into `input_audio.append` of at least 3200 bytes (100 ms) and at most 262144 bytes                                                 | The docs sample uses 100 ms chunks; 262144 is the documented maximum. The rest is flushed before the end.                   |
| Connect timing  | Audio pushed while the socket connects waits in order and is sent after `session.update`; the next session is opened in the background after each utterance   | A new utterance finds a ready socket. A spare the server closes before use is replaced without an error.                    |
| Partials, final | `transcription.text.delta` builds the partial text; the text of `transcription.done` is the final. Empty text gives no final, only the finalize marker        | Same contract as the other vendors: every finalize gets exactly one marker, in request order.                               |
| Failures        | `error` gives `mistral stt error <code>: <message>`; a close or read failure gives `mistral stt websocket closed`; no `done` within 8 s gives a named timeout | One error per utterance, then the finalize marker. The next push opens a new session. Transcript text is never in an error. |
| Ignored         | `session.created`, `session.updated`, `transcription.language`, `transcription.segment`, Ping/Pong, unknown types                                             | They never end the reader.                                                                                                  |
| Not used        | `input_audio.flush`, `turn_detection`, diarization, context bias                                                                                              | Flush semantics are undocumented, and the VAD of this library owns turn detection.                                          |
| Cost            | Billed per streamed minute, so presets use `vad.gateStt: true`                                                                                                | Same as Fish Audio.                                                                                                         |

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-mistral --features live --lib --test live_endpoints --test stt_mock_server_test --test tts_mock_server_test
```

Real-network tests (feature `live`, key in `MISTRAL_API_KEY`; `MISTRAL_LIVE_REQUIRED=1` turns a missing key into a failure):

```bash
MISTRAL_API_KEY=... cargo test -p node-webrtc-rust-vendor-mistral --features live --test live_endpoints --test live_test -- --nocapture --test-threads=1
```

- `live_test`: TTS first audio and decoded length for both TTS models, and a VAD-gated realtime STT round trip on the counting phrase. They honour `MISTRAL_BASE_URL`.
- `live_endpoints`: for each documented endpoint, the documented `GET /v1/models` (which of our model ids it serves) and the median of 5 TCP+TLS connect times, then the choice: of the endpoints serving the default TTS and STT models, the lowest median connect; else the lowest one serving the default STT model plus any TTS model. No billed call is made.
- `live_bench`: latency legs for the bench (`vendor-bench.yml`, vendor `mistral`): both TTS models, and STT with `target_streaming_delay_ms` 160, 240 and 480 (the model field reads `<model>@delay<ms>`).
