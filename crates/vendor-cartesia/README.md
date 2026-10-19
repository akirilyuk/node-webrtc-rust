# vendor-cartesia

Cartesia TTS only (no STT) for `@node-webrtc-rust/sdk` VoiceAgent.

## Docs

|               | URL                                                               |
| ------------- | ----------------------------------------------------------------- |
| WebSocket TTS | [WebSocket](https://docs.cartesia.ai/api-reference/tts/websocket) |
| REST bytes    | [Bytes](https://docs.cartesia.ai/api-reference/tts/bytes)         |
| Models        | [Models](https://docs.cartesia.ai/models)                         |

## Environment

| Variable            | Use                                                                      |
| ------------------- | ------------------------------------------------------------------------ |
| `CARTESIA_API_KEY`  | Cartesia API key                                                         |
| `CARTESIA_VOICE_ID` | Voice id, used when `tts.voice` is unset. Cartesia has no default voice. |

Neither `tts.voice` nor `CARTESIA_VOICE_ID` set is a config error (`set tts.voice or CARTESIA_VOICE_ID to a Cartesia voice id`), raised before any request.

### Endpoint override

`TtsConfig.endpoint` (for example `http://127.0.0.1:<port>`) replaces `https://api.cartesia.ai`. REST becomes `{endpoint}/tts/bytes` and WebSocket becomes `ws://…/tts/websocket?cartesia_version=…` (`https` maps to `wss`, `http` to `ws`). Unset keeps the default host. The local mock-server tests use this.

### Timeouts

Connect 10 s (WebSocket handshake: `cartesia tts websocket connect timed out after 10 s`; HTTP connect), first audio 15 s (WebSocket and HTTP), between chunks 10 s, every WebSocket send 10 s, REST total 60 s. A WS failure or first-audio timeout before any audio reached the sink falls back to the streamed REST body; a stall after audio is an error.

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

TTS `websocket`: the contexts socket, with the streamed REST fallback only while no audio reached the sink. TTS `http`: `POST /tts/bytes`, streamed.

**HTTP streams from `/tts/bytes`.** Cartesia's endpoint comparison says "All three endpoints stream audio as it is produced" (bytes delivers one HTTP response body; SSE and WebSocket chunk it into events or messages). The bytes endpoint's own page only says "Stream audio from a complete transcript" and lists the response as `audio/*` binary. We use `/tts/bytes` and read the body as it arrives (`Response::chunk`): raw `pcm_s16le` at 48 kHz has no framing to parse, which keeps one request shape for the WebSocket fallback and for `http`. `/tts/sse` is also documented ("Server-sent events stream. Each frame is `data: <json>\n\n`") but adds base64 and JSON framing for no gain here, so it is not used. Each body chunk goes to the sink once, an odd trailing byte is carried to the next chunk so every frame is sample-aligned, a body that errors or ends early after audio is an error with no retry, and an empty body is an error (`Cartesia bytes returned no audio`).

`auto` resolves to the catalog default, which is today's behaviour: TTS `websocket`. Default measured in a later change.

HTTP requests use one pooled client per provider; HTTP/2 when the vendor negotiates it (ALPN), otherwise HTTP/1.1.

### WebSocket reuse across utterances

One TTS socket is kept per `CartesiaTts` and reused for the next utterance, each utterance on a fresh `context_id` (`continue: false`). Basis in the docs: "WebSocket amortizes that cost when you keep the socket open" (endpoint comparison, "long-lived sessions with partial input, multiple utterances") and "The server will send you back a stream of messages with the same `context_id` as your request" (WebSocket reference); messages of another `context_id` are ignored. The docs give no idle timeout or maximum connection age for the TTS WebSocket, so the socket is reused only if it finished an utterance within 60 s (our bound), and is replaced when the server closed it. A failure on a reused socket before any audio arrived may retry once on a fresh socket; after audio an error is returned and nothing is replayed. The socket is never reused after an error or a cancel. A cancel sends the documented `{"context_id": "...", "cancel": true}` and then closes the socket.

### Live tests and bench

```bash
# from node-webrtc-rust/
CARTESIA_API_KEY=... CARTESIA_VOICE_ID=... cargo test -p node-webrtc-rust-vendor-cartesia --features live --test live_test -- --nocapture --test-threads=1
NWR_BENCH_RUNS=3 CARTESIA_API_KEY=... CARTESIA_VOICE_ID=... cargo test -p node-webrtc-rust-vendor-cartesia --features live --test live_bench -- --nocapture --test-threads=1
```

Without both variables they skip; `CARTESIA_LIVE_REQUIRED=1` turns a missing one into a panic that names it (CI sets it, `.github/workflows/vendor-live-cartesia.yml`; secrets `CARTESIA_LIVE_KEY` and `CARTESIA_LIVE_VOICE_ID`). The bench runs through `vendor-bench.yml`. The mock-server tests run in PR integration (`cargo vendor-cartesia`).
