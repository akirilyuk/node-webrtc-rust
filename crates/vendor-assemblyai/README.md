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
cargo test -p node-webrtc-rust-vendor-assemblyai --lib
```

## Transport

Config: `stt.transport`: `auto` (default) or `websocket`. AssemblyAI has no TTS. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

STT is always the streaming WebSocket.

`auto` resolves to the catalog default, which is today's behaviour: STT `websocket`. Default measured in a later change.

There is no HTTP request path in this crate, so no HTTP client or HTTP/2 setting applies. `last_transport()` reports `ws` once the streaming socket is open.
