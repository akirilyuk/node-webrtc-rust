# vendor-fishaudio

Fish Audio TTS and realtime STT for `@node-webrtc-rust/sdk` VoiceAgent. Default TTS model: **Drama 3** (`drama-3`). STT model: `transcribe-1`.

## Docs

|                | URL                                                                                                              |
| -------------- | ---------------------------------------------------------------------------------------------------------------- |
| WebSocket TTS  | [tts-live](https://docs.fish.audio/api-reference/endpoint/websocket/tts-live)                                    |
| HTTP TTS       | [text-to-speech](https://docs.fish.audio/api-reference/endpoint/openapi-v1/text-to-speech)                       |
| Drama 3        | [drama-3](https://docs.fish.audio/features/text-to-speech/drama-3)                                               |
| Realtime guide | [realtime-streaming](https://docs.fish.audio/features/realtime-streaming)                                        |
| Models         | [models-overview](https://docs.fish.audio/developer-guide/models-pricing/models-overview)                        |
| Rate limits    | [pricing-and-rate-limits](https://docs.fish.audio/developer-guide/models-pricing/pricing-and-rate-limits)        |
| Realtime STT   | [migrate-from-elevenlabs](https://docs.fish.audio/developer-guide/compat/migrate-from-elevenlabs) (realtime STT) |

## Environment

| Variable        | Use                                                                                                    |
| --------------- | ------------------------------------------------------------------------------------------------------ |
| `FISH_API_KEY`  | Fish Audio API key (`Authorization: Bearer` for TTS, `xi-api-key` for the STT socket)                  |
| `FISH_VOICE_ID` | Voice `reference_id` used when `tts.voice` is unset, empty or `default`. Unset: `reference_id` omitted |

`tts.endpoint` / `stt.endpoint` replace the API origin (default `https://api.fish.audio`). The WebSocket URLs are derived from it (`https` to `wss`, `http` to `ws`).

## Streaming decisions (TTS)

| Choice        | What we implemented                                                                                                                                                                                  | Why                                                                                                                                                                                                                                                                    |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Default       | Chunked HTTP `POST /v1/tts`                                                                                                                                                                          | Lower time to first audio for sentence-level text: measured 291-658 ms vs 963-1035 ms over WS, 2026-10-16.                                                                                                                                                             |
| Fallback      | WS `/v1/tts/live`, MessagePack frames (`start`, `text`, `flush`, `stop`), only before the first chunk reached the sink, and only after a connect or transport error, a timeout, HTTP 429 or HTTP 5xx | Documented streaming path. After audio started, a retry would play the start of the utterance twice, so the error wins. An HTTP 4xx (400, 401, 402, 403, 404: bad reference, key or credit) fails the same way on the WebSocket 15 s later, so it is returned at once. |
| Audio format  | `format: "pcm"`, `sample_rate: 24000`, mono s16le, upsampled x2 to 48 kHz stereo                                                                                                                     | 48 kHz is not documented for PCM. Same approach as the OpenAI and Groq vendors.                                                                                                                                                                                        |
| Latency       | `latency: "balanced"`                                                                                                                                                                                | Documented as the lowest time to first audio, meant for voice agents.                                                                                                                                                                                                  |
| Normalization | `normalize: true`                                                                                                                                                                                    | Documented request field.                                                                                                                                                                                                                                              |
| Model         | `model` header, validated against the catalog before anything is sent                                                                                                                                | Fish silently falls back to `s2.1-pro` on a missing or misspelled `model`. Unknown ids are a config error.                                                                                                                                                             |
| Voice         | `reference_id` from `tts.voice`, else `FISH_VOICE_ID`, else omitted                                                                                                                                  | `reference_id` is nullable in the docs.                                                                                                                                                                                                                                |
| Bounds        | connect 10 s, first audio 15 s, idle between chunks 10 s, HTTP total 60 s                                                                                                                            | Every wait has a named error (`fishaudio http first audio timed out after 15000 ms`).                                                                                                                                                                                  |
| Cancel        | Sink cancel closes the socket (WS) or drops the body (HTTP)                                                                                                                                          | Barge-in stops reading at the next frame.                                                                                                                                                                                                                              |
| Sink contract | Every PCM chunk goes to the sink once, as it arrives                                                                                                                                                 | The returned chunk is the whole utterance for the phrase cache and is never re-sent to the sink.                                                                                                                                                                       |

Only these request fields are sent: `text`, `format`, `sample_rate`, `latency`, `normalize`, `reference_id`. Request text and audio are never logged and never appear in error messages.

### Drama 3 text

Direction tags (`[nervous, quick]`), sound tags (`[small laugh]`) and paired tags (`<whisper>...</whisper>`) go in `text` exactly as written. The text is passed through untouched.

### Concurrency

Fish limits concurrent requests per tier: 5 (Starter), 15 (Elevated), 50 (High Volume). There is no client-side limiter; a request over the limit surfaces Fish's error.

TTS and STT share the account's concurrency slots, so a call with both open counts twice.

## Realtime STT

Model `transcribe-1`, over Fish's ElevenLabs-compatible realtime socket (the only Fish STT path that takes streamed audio; `POST /v1/asr` takes a finished file). `FishAudioStt` is a `FinalizeMode::Deferred` provider.

| Choice       | What we implemented                                                                                                                                                                                                                | Why                                                                                                                               |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Socket       | `{ws_base}/compat/elevenlabs/v1/speech-to-text/realtime?model_id=fish-audio/transcribe-1&commit_strategy=manual&audio_format=pcm_16000`                                                                                            | Documented in the compat guide. `commit_strategy=vad` is refused at the handshake, so Fish never decides where an utterance ends. |
| Auth         | header `xi-api-key: <FISH_API_KEY>`                                                                                                                                                                                                | As in the compat guide's examples.                                                                                                |
| Audio        | 16 kHz mono s16le, base64 in `input_audio_chunk` frames with `commit: false`, `sample_rate: 16000`                                                                                                                                 | A wrong `audio_format` gives a confident wrong transcript instead of an error, so the format is pinned, not configurable.         |
| Commit       | `request_finalize` sends an empty `input_audio_chunk` with `commit: true`                                                                                                                                                          | Fish returns a transcript only after a commit. The VoiceAgent commits at the end of the VAD gate hold.                            |
| Answer       | `committed_transcript` becomes `Final(text)` (only if the text is not empty) followed by `FinalizeDone`                                                                                                                            | Exactly one `FinalizeDone` per commit, in order. `committed_transcript_with_timestamps` is ignored.                               |
| Errors       | A `message_type` ending in `error` is surfaced as one error and answers the oldest pending commit                                                                                                                                  | A commit never goes without a terminal marker. Unknown message types are ignored.                                                 |
| Socket close | With a commit pending or audio sent since the last commit: one error and `FinalizeDone` for every pending commit. An idle close (nothing lost) raises no error. Either way the next push or commit reconnects (connect bound 10 s) | Nothing is replayed: audio sent on the dead socket is gone. A failed reconnect returns the error to the caller.                   |
| 3600 s cap   | After 3300 s of connection age, the next push moves to a fresh socket if nothing is pending and no utterance is in progress                                                                                                        | A socket lives at most 3600 s. Replacing it mid-utterance would split the audio, so that case waits for the commit.               |
| Stop         | Sends a close frame under a 1 s bound and never waits for the server                                                                                                                                                               | Teardown is best effort.                                                                                                          |
| Language     | None sent                                                                                                                                                                                                                          | The socket documents no `language_code`; Fish auto-detects.                                                                       |

What this means for a VoiceAgent:

- **No partials.** Fish returns one final per commit, so `user_speech_partial` never fires. The provider reports `emits_partials() == false`, and the agent adapts without configuration: C1 (`sttListenTimeoutMs`, no partial after `vad_triggered`) is not armed, so a long utterance is no longer cut at 4 s and its final is delivered; `bargeIn.requireSttPartial` (under `vad.bargeIn`) is treated as `false`, so barge-in during agent speech uses VAD alone. This is read when the user starts speaking, so `updateStt` to or from Fish takes effect mid-session. The final arrives after the VAD end plus Fish's processing time; an empty commit still ends the turn with `user_stt_not_found`.
- **Set `vad.gateStt: true`** (`VOICE_AGENT_VAD_PRESET`). STT is billed for the audio streamed; with the gate only speech and its hold window reach Fish. Streaming continuously is the caller's choice and is billed accordingly.
- No client-side limiter. Over the tier's concurrency limit Fish refuses the open and the error surfaces.
- Transcript text is never logged and never appears in an error message.

### Long turns

Fish STT sends no partial transcripts, so nothing is transcribed while the user speaks. The turn is committed when VAD sees a pause (`vad.minSilenceDurationMs`, default 1300 ms, plus the gate hold), and the final arrives about 0.5 s later. There is no maximum utterance length: a user who talks without pausing gets one transcript when they stop, and the agent's reply waits for it. Audio is still streamed live, so the commit does not upload anything extra. Fish limits buffered audio per commit to 100 MiB (about 54 minutes at 16 kHz). Use a streaming STT vendor if you need live text.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-fishaudio
cargo test -p node-webrtc-rust-vendor-fishaudio --features live   # includes the mock-server tests (TTS and STT)
```

`tests/vad_gate_test.rs` checks the VAD gate used by the live STT test and runs without the `live` feature. `tests/live_test.rs` needs `FISH_API_KEY`; `FISH_LIVE_REQUIRED=1` turns a missing key into a failure.

## Transport

Config: `tts.transport`: `auto` (default), `http` or `websocket`. `stt.transport`: `auto` or `websocket`. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

TTS `http` (chunked `POST /v1/tts`, with the documented WebSocket fallback while no audio reached the sink and the failure is one the socket can fix). TTS `websocket` (`/v1/tts/live` only, no fallback). STT is always the realtime WebSocket.

`auto` resolves to the catalog default, which is today's behaviour: TTS `http`, STT `websocket`. Default measured in a later change.
