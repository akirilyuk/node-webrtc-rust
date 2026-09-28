# node-webrtc-rust-vendor-openai

OpenAI STT/TTS adapter for VoiceAgent (`SttVendor::Openai` / `TtsVendor::Openai`). Live HTTP and WebSocket calls sit behind the crate `live` feature (enabled by the native bindings).

Canonical routing is the hardcoded table in [`src/matrix.rs`](./src/matrix.rs) (no `GET /v1/models`). Unknown STT model names fail config with the documented list below.

Official docs:

- [Transcription](https://platform.openai.com/docs/guides/transcription)
- [File transcription](https://platform.openai.com/docs/guides/speech-to-text)
- [Realtime transcription](https://platform.openai.com/docs/guides/realtime-transcription)
- [Realtime WebSockets](https://platform.openai.com/docs/guides/realtime-websocket)
- [Realtime WebRTC](https://platform.openai.com/docs/guides/realtime-webrtc)
- [Create speech](https://platform.openai.com/docs/api-reference/audio/createSpeech)

## Why Realtime uses WebSocket (not a second WebRTC hop)

OpenAI documents **two** Realtime transports:

| Transport | OpenAI’s intended client |
|-----------|--------------------------|
| WebSocket | Server that already owns PCM (`input_audio_buffer.append` / `commit`) |
| WebRTC | Browser or mobile talking to OpenAI (`POST /v1/realtime/calls` SDP) |

This crate is the first kind. The customer already has a WebRTC hop into the host. VoiceAgent then runs VAD, optional RNNoise, mix, recording, language ID, and barge-in on **in-process PCM**. STT sees 16 kHz mono after that pipeline.

We do **not** forward the customer media track to OpenAI as another WebRTC peer:

1. **The first hop is already paid.** Decode from the customer is done. A new peer to OpenAI adds ICE, DTLS, SRTP, and SDP. From a typical worker (NAT, often TURN) that is slower to start and easier to break than `wss://api.openai.com/v1/realtime` after `POST /v1/realtime/client_secrets`.
2. **RTP vs WebSocket is not the STT bottleneck.** After the session is up, both send 24 kHz mono. JSON/base64 on the socket is extra **bytes**, not extra model time. Utterance delay is VAD hold, commit, and the transcribe model — the same on both transports. OpenAI’s WebRTC “faster / more consistent” guidance is about **client** networks (jitter, NAT), not a server that already has PCM.

   **How much JSON/base64 actually is** (Realtime live only; VoiceAgent pushes ~20 ms frames, then we resample 16 kHz → 24 kHz `pcm16` and `input_audio_buffer.append`):

   | | Per 20 ms frame | Per second of gated speech |
   |---|----------------:|---------------------------:|
   | Raw PCM (24 kHz mono s16le) | 960 B | 48 KB/s (384 kbit/s) |
   | Base64 of that PCM | 1280 B (+33.3%) | 64 KB/s |
   | JSON wrapper (`{"type":"input_audio_buffer.append","audio":…}`) | 47 B | 2.4 KB/s |
   | JSON body on the wire | 1327 B (**+38%** vs PCM) | 66.4 KB/s (531 kbit/s) |

   Almost all of the expansion is **base64** (fixed 4/3). The JSON keys are 47 bytes per message — about 4% of a 20 ms frame, ~1.5% if we batched to 100 ms, and noise if we batched to 1 s. CPU to base64-encode 48 KB/s is microseconds; it is not a session-cost line.

   OpenAI bills Realtime transcription by **audio duration**, not by encoded bytes, so JSON/base64 does not change the vendor invoice. The leftover cost is **our egress**: ~1.1 MB extra per minute of gated speech (~4.0 MB/min JSON vs 2.9 MB/min raw PCM). At typical cloud egress (~$0.09/GB) that is ~$0.0001 per minute — on the order of **$6 per 1000 hours** of Realtime speech, not per 1000 hours of wall-clock sessions (file-default models never pay this).

   WebRTC’s large bandwidth win would be **Opus** (often ~16–40 kbit/s), not dropping JSON. That is codec compression plus a second PeerConnection, not a cheaper encoding of the same PCM. We still decode locally for VAD/mix/recording, so a raw forward does not remove the first hop.

3. **A raw forward skips this stack.** OpenAI WebRTC uses SDP-negotiated audio; you must not send `input_audio_buffer.append` on the data channel. Mix, recording, and barge-in all assume `SttProvider` PCM. Forking the track still means decode locally **and** ship RTP.
4. **Most VoiceAgent OpenAI STT is not Realtime.** Default for `gpt-4o-mini-transcribe` (and other file models) is `POST /v1/audio/transcriptions`. WebRTC to OpenAI only applies to Realtime models.

Browser WebRTC **directly** to OpenAI is a different product (ephemeral key in the client, media not going through VoiceAgent). It is not a drop-in for this vendor.

Realtime connect here: `POST /v1/realtime/client_secrets` with `session.type=transcription`, then `wss://api.openai.com/v1/realtime` with Bearer `ek_…`.

## STT model matrix

File SSE streams text of a **completed** utterance WAV. It is not live-mic Realtime. `whisper-1` does not support `stream=true`.

| Model | File JSON | File SSE (`stream=true`) | Realtime live | Realtime committed-turn | VoiceAgent default |
|-------|:---------:|:------------------------:|:-------------:|:-----------------------:|--------------------|
| `whisper-1` | yes | no | no | no | File JSON |
| `gpt-4o-mini-transcribe` | yes | yes | no | no | File SSE |
| `gpt-4o-transcribe` | yes | yes | no | no | File SSE |
| `gpt-4o-transcribe-diarize` | yes | yes | no | no | File SSE |
| `gpt-transcribe` | yes | yes | no | yes (specialized) | File SSE |
| `gpt-live-transcribe` | no | no | yes | commit still used with our VAD | Realtime live |

- **File JSON** — `POST /v1/audio/transcriptions`, `stream` omitted; response `{ "text": "…" }`.
- **File SSE** — same POST with `stream=true`; `transcript.text.delta` then `transcript.text.done`.
- **Realtime live** — append 24 kHz PCM while the VAD gate is open; `input_audio_buffer.commit` on `finalize_utterance`; events `conversation.item.input_audio_transcription.delta` / `.completed`.
- **Realtime committed-turn** — same WebSocket; transcription starts after commit (`gpt-transcribe`). Implemented for tests; not the VoiceAgent default (bounded utterances use file SSE).

`finalize_utterance` starts the documented request (file POST or WS commit) and returns. `poll_transcript` waits while that request is in flight so VoiceAgent can drain partials then a final without awaiting inside finalize.

## TTS (brief)

VoiceAgent default is `stream_format=audio` (`response_format=pcm`) with a full-body fallback. `stream_format=sse` (`speech.audio.delta` / `speech.audio.done`) is implemented for `gpt-4o-mini-tts` and must not be sent for `tts-1` / `tts-1-hd`.

## Tests

```bash
cargo test -p node-webrtc-rust-vendor-openai --lib
cargo test -p node-webrtc-rust-vendor-openai --features live --lib
OPENAI_LIVE=1 OPENAI_API_KEY=sk-... cargo test -p node-webrtc-rust-vendor-openai --features live --test live_openai_matrix
```

`src/matrix.rs` unit tests assert every `DOCUMENTED_STT_MODELS` row (default transport + `stt_file_sse_supported`) and TTS SSE deny for `tts-1` / `tts-1-hd`.

Live matrix (`tests/live_openai_matrix.rs`, `OPENAI_LIVE=1`):

| Matrix cell | Live test |
|-------------|-----------|
| STT default transport (all six models) | `live_stt_default_transport_every_documented_model` |
| STT file JSON (`new_force_http_file_json`) for SSE-capable file models | `live_stt_file_json_sse_capable_models` |
| STT Realtime committed-turn (`gpt-transcribe`, `new_force_realtime`) | `live_stt_realtime_committed_gpt_transcribe` |
| TTS `stream_format=audio` + progressive (`tts-1`, `tts-1-hd`, `gpt-4o-mini-tts`) | `live_tts_audio_and_progressive` |
| TTS SSE allow (`gpt-4o-mini-tts`) / deny (`tts-1`, `tts-1-hd`) | `live_tts_sse_allow_and_deny` |

Optional: `gpt-4o-mini-tts-2025-12-15` is exercised inside `live_tts_audio_and_progressive` when the API accepts that model id.
