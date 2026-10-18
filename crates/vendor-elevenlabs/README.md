# vendor-elevenlabs

ElevenLabs Scribe realtime STT and TTS for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|                     | URL                                                                                                            |
| ------------------- | -------------------------------------------------------------------------------------------------------------- |
| Scribe realtime STT | [Speech to text realtime](https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime) |
| TTS HTTP stream     | [Stream](https://elevenlabs.io/docs/api-reference/text-to-speech/stream)                                       |
| TTS WebSocket       | [Realtime TTS websockets](https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts)         |

## Environment

| Variable             | Use                        |
| -------------------- | -------------------------- |
| `ELEVENLABS_API_KEY` | `xi-api-key` for STT + TTS |

`config.apiKey` overrides the env var. TTS `config.voice` is the ElevenLabs `voice_id`.

### Endpoint override

`TtsConfig.endpoint` (for example `http://127.0.0.1:<port>`) replaces `https://api.elevenlabs.io` for TTS. HTTP stream becomes `{endpoint}/v1/text-to-speech/{voice_id}/stream?…`, full body `{endpoint}/v1/text-to-speech/{voice_id}?…`, and the WebSocket base maps `https` to `wss` and `http` to `ws`. Unset keeps the default host. The local mock-server tests use this. STT is unaffected.

## Streaming decisions (STT)

| Choice    | What we implemented                                                    | Why                                                                                          |
| --------- | ---------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Transport | `wss://api.elevenlabs.io/v1/speech-to-text/realtime`                   | Official Scribe v2 realtime API.                                                             |
| Model     | `scribe_v2_realtime` only                                              | Documented realtime model. File Scribe (`scribe_v2`) is **not** this socket and is rejected. |
| Commit    | `commit_strategy=manual` + `input_audio_chunk` on `finalize_utterance` | Matches VoiceAgent VAD: we own utterance boundaries.                                         |
| Events    | `partial_transcript` → partial; `committed_transcript` → final         | Official event names.                                                                        |

## Streaming decisions (TTS)

| Choice                   | What we implemented                                          | Why                                                                                               |
| ------------------------ | ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------- |
| Default progressive      | HTTP `POST …/text-to-speech/{voice_id}/stream` (`pcm_48000`) | Official stream API; works for `eleven_multilingual_v2`, Flash, etc.                              |
| WebSocket `stream-input` | Implemented and **allowed** except `eleven_v3`               | Official realtime TTS WS.                                                                         |
| `eleven_v3`              | Full-body `POST …/text-to-speech/{voice_id}` only            | **Documented:** `stream-input` does not support `eleven_v3`. We do not send that model on the WS. |

The WebSocket `stream-input` path is selected with the `websocket` customer transport (never for `eleven_v3`). If the HTTP stream yields no audio, the full-body POST is used as a fallback and its audio reaches the sink once; a cancelled utterance never falls back.

The WebSocket path treats a close frame, a closed stream or a socket error before `{"isFinal": true}` as an error (the audio received so far has already reached the sink), and it is bounded like the HTTP path (connect and sends 10 s, first audio 15 s, between messages 10 s). `ElevenLabsTts::synthesize_websocket_stream` is public so the mock-server tests can call it directly.

Default TTS model: `eleven_multilingual_v2`.

### Timeouts

Connect 10 s, first audio (response headers and first body chunk) 15 s, between body chunks 10 s, full-body POST total 60 s. The HTTP stream has no total timeout, so a long utterance is not cut.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-elevenlabs --lib
cargo test -p node-webrtc-rust-vendor-elevenlabs --features live --test mock_server_test
```

## Transport

Config: `tts.transport`: `auto` (default), `http` or `websocket`. `stt.transport`: `auto` or `websocket`. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

TTS `http` (chunked `…/stream`, or the full-body POST for `eleven_v3`) and TTS `websocket` (`…/stream-input`). `websocket` is rejected at construction for models the socket does not support (`eleven_v3`). STT is always the Scribe realtime WebSocket.

`auto` resolves to the catalog default, which is today's behaviour: TTS `http`, STT `websocket`. Default measured in a later change.

### WebSocket streaming and reuse

`stream-input` forwards every `audio` message to the sink as it arrives (each chunk once) and ends on `isFinal` ("Indicates if the generation is complete"). A close frame, a closed stream or a read error before `isFinal` is an error. One socket per utterance is kept on purpose: the sequence is `InitializeConnection` (`" "`), `{"text": ..., "flush": true}`, then the documented closing message (empty text) "to close the WebSocket connection after sending the last text message". The multi-context socket (`multi-stream-input`, `context_id`, `close_context`, `isFinal` "for the context") is a reference page only; the model list it supports and its per-context end marker are not published in a form that can be checked, so it is not used, and the socket is not reused. The connection also closes after 20 s of inactivity by default (`inactivity_timeout`).

### Connect bound

Every WebSocket connect (TTS and STT) is bounded at 10 s with the error `elevenlabs <tts|stt> websocket connect timed out after 10 s`; each control send is bounded at 10 s (`elevenlabs tts websocket send timed out after 10 s`).

### Live tests and bench

```bash
# from node-webrtc-rust/
ELEVENLABS_API_KEY=... cargo test -p node-webrtc-rust-vendor-elevenlabs --features live --test live_test -- --nocapture --test-threads=1
NWR_BENCH_RUNS=3 ELEVENLABS_API_KEY=... cargo test -p node-webrtc-rust-vendor-elevenlabs --features live --test live_bench -- --nocapture --test-threads=1
```

Without the key both skip; `ELEVENLABS_LIVE_REQUIRED=1` turns a missing key into a panic (CI sets it, `.github/workflows/vendor-live-elevenlabs.yml`). The bench runs through `vendor-bench.yml`.

HTTP requests use one pooled client per provider; HTTP/2 when the vendor negotiates it (ALPN), otherwise HTTP/1.1.
