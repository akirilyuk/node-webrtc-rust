# vendor-assemblyai

AssemblyAI streaming STT only (no TTS) for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|               | URL                                                                   |
| ------------- | --------------------------------------------------------------------- |
| Streaming STT | [Streaming](https://www.assemblyai.com/docs/speech-to-text/streaming) |

## Environment

| Variable             | Use                      |
| -------------------- | ------------------------ |
| `ASSEMBLYAI_API_KEY` | Streaming WebSocket auth |

## Streaming decisions (STT)

| Choice    | What we implemented                                          | Why                                                            |
| --------- | ------------------------------------------------------------ | -------------------------------------------------------------- |
| Transport | `wss://streaming.assemblyai.com/v3/ws`                       | Official v3 streaming WebSocket.                               |
| Models    | `universal-streaming-english` (default), `universal-3-6-pro` | Documented `speech_model` query values.                        |
| Events    | v3 `Turn` (`transcript`, `end_of_turn`)                      | Official message shape. Partials while `end_of_turn` is false. |
| Encoding  | `pcm_s16le` @ 16 kHz                                         | Matches VoiceAgent STT PCM.                                    |

**Not implemented:** pre-recorded / file transcription REST. This crate is live streaming only.

TTS: none. Pair with a TTS vendor.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-assemblyai --lib --test mock_server_test --test vendor_test
```

## Transport

Config: `stt.transport`: `auto` (default) or `websocket`. AssemblyAI has no TTS. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

STT is always the streaming WebSocket.

`auto` resolves to the catalog default, which is today's behaviour: STT `websocket`. Default measured in a later change.

There is no HTTP request path in this crate, so no HTTP client or HTTP/2 setting applies. `last_transport()` reports `ws` once the streaming socket is open.

### Streaming behaviour

- **Partials.** The docs say "Expect several partial updates before each `end_of_turn: true`", so `Turn` messages with `end_of_turn: false` are surfaced as `SttTranscript::Partial` and `end_of_turn: true` as `Final`; `emits_partials()` stays `true` (the live test asserts it matches what arrives).
- **Finalize.** `finalize_utterance` sends the documented `{"type":"ForceEndpoint"}` ("Manually force an endpoint in the transcription."). It ends the open turn and keeps the session. `stop()` sends `{"type":"Terminate"}` ("Terminate finalizes the open turn") and waits at most 3 s for the socket task before aborting it.
- **One session, several utterances.** The docs describe several turns per session and "a 3-hour maximum session duration", so a second utterance reuses the open socket (live test `assemblyai_stt_live_second_utterance_on_one_session`; the bench adds a `warm` sample for it).
- **Bounds.** WebSocket connect 10 s (`assemblyai stt websocket connect timed out after 10 s`) and every send on the socket 10 s (`assemblyai stt websocket send timed out after 10 s`). A socket that closed or got stuck ends the session task; the next `push_audio` returns `assemblyai stt websocket closed`. A Ping/Pong frame no longer ends the session.
- **Endpoint.** `stt.endpoint` replaces the host (`https://` maps to `wss://`, `http://` to `ws://`, path `/v3/ws`), for the regional streaming hosts in the docs and for the local mock server.

### Live tests and bench

AssemblyAI has no TTS, so the audio is the committed Sherpa fixture (`vendor_live_support::stt_fixture_stereo_48k`). The `live` cargo feature only gates the live tests; the client is always compiled.

```bash
# from node-webrtc-rust/
ASSEMBLYAI_API_KEY=... cargo test -p node-webrtc-rust-vendor-assemblyai --features live --test live_test -- --nocapture --test-threads=1
NWR_BENCH_RUNS=3 ASSEMBLYAI_API_KEY=... cargo test -p node-webrtc-rust-vendor-assemblyai --features live --test live_bench -- --nocapture --test-threads=1
```

Without the key both skip; `ASSEMBLYAI_LIVE_REQUIRED=1` turns a missing key into a panic (CI sets it, `.github/workflows/vendor-live-assemblyai.yml`, secret `ASSEMBLYAI_LIVE_KEY`). The bench runs through `vendor-bench.yml`. The mock-server tests run in PR integration (`cargo vendor-assemblyai`).
