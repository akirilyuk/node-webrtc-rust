# vendor-cartesia

Cartesia TTS only (no STT) for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

| | URL |
|---|---|
| WebSocket TTS | [WebSocket](https://docs.cartesia.ai/api-reference/tts/websocket) |
| REST bytes | [Bytes](https://docs.cartesia.ai/api-reference/tts/bytes) |
| Models | [Models](https://docs.cartesia.ai/models) |

## Environment

| Variable | Use |
|---|---|
| `CARTESIA_API_KEY` | Cartesia API key |

## Streaming decisions (TTS)

| Choice | What we implemented | Why |
|---|---|---|
| Default | WS `/tts/websocket` contexts (`chunk` / `done`) | Official WebSocket API; progressive PCM for VoiceAgent. |
| Fallback | REST `/tts/bytes` | Documented non-WS path if WS is unavailable. |
| Models | `sonic-3` (default), `sonic-3.5`, `sonic-3.6`, `sonic-latest` | Current WS `model_id` enum. Legacy `sonic-english` is **not** accepted. |
| API version | `2026-08-14` (`cartesia_version` query) | Documented AsyncAPI version at implementation time. |

STT: this vendor is TTS-only. Pair with another STT provider.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-cartesia --lib
```
