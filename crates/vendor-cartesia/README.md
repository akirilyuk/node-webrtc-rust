# vendor-cartesia

Cartesia TTS only (no STT) for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|               | URL                                                               |
| ------------- | ----------------------------------------------------------------- |
| WebSocket TTS | [WebSocket](https://docs.cartesia.ai/api-reference/tts/websocket) |
| REST bytes    | [Bytes](https://docs.cartesia.ai/api-reference/tts/bytes)         |
| Models        | [Models](https://docs.cartesia.ai/models)                         |

## Environment

| Variable           | Use              |
| ------------------ | ---------------- |
| `CARTESIA_API_KEY` | Cartesia API key |

### Endpoint override

`TtsConfig.endpoint` (for example `http://127.0.0.1:<port>`) replaces `https://api.cartesia.ai`. REST becomes `{endpoint}/tts/bytes` and WebSocket becomes `ws://…/tts/websocket?cartesia_version=…` (`https` maps to `wss`, `http` to `ws`). Unset keeps the default host. The local mock-server tests use this.

### Timeouts

Connect 10 s, first audio 15 s, between chunks 10 s, REST total 60 s. A WS first-audio timeout falls back to REST (nothing reached the sink); a stall after audio is an error.

## Streaming decisions (TTS)

| Choice      | What we implemented                                                                                                            | Why                                                                     |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------- |
| Default     | WS `/tts/websocket` contexts (`chunk` / `done`)                                                                                | Official WebSocket API; progressive PCM for VoiceAgent.                 |
| Fallback    | REST `/tts/bytes` only when the WS failed before any audio reached the sink; a WS failure after audio is returned as an error. | Falling back after audio would play the utterance twice.                |
| Models      | `sonic-3` (default), `sonic-3.5`, `sonic-3.6`, `sonic-latest`                                                                  | Current WS `model_id` enum. Legacy `sonic-english` is **not** accepted. |
| API version | `2026-08-14` (`cartesia_version` query)                                                                                        | Documented AsyncAPI version at implementation time.                     |

STT: this vendor is TTS-only. Pair with another STT provider.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-cartesia --lib
cargo test -p node-webrtc-rust-vendor-cartesia --features live --test mock_server_test
```

## Transport

Config: `tts.transport`: `auto` (default), `websocket` or `http`. Cartesia has no STT. Allowed values per leg come from `packages/voice-catalog/catalog/vendors.json` (`ttsTransports` / `sttTransports`); an unsupported value fails at provider construction with the allowed list.

TTS `websocket` (contexts socket, with the existing REST fallback only while no audio reached the sink). TTS `http` (REST bytes only, the body is read in full before it is sent to the sink today).

`auto` resolves to the catalog default, which is today's behaviour: TTS `websocket`. Default measured in a later change.
