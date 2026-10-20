# OpenAI on node-webrtc-rust 0.12.2, Hetzner EU, 2026-10-20

The OpenAI rows of the library results, re-measured on 0.12.2 (vendor-bench.yml run 39386103894,
`main` at `0c7213a`, the 0.12.2 release commit). Same runner, inputs and 10 runs as
[`../2026-10-19-ci-eu/`](../2026-10-19-ci-eu/); only OpenAI ran.

- TTS (`tts-1`, HTTP): the crate buffers 250 ms of audio before playback starts, so
  `playout_start_ms` (when playback starts) is the number a listener notices; `ttfa_ms` is the first
  chunk. No sample stalled (`stall_ms` 0 in all 40).
- STT: `whisper-1` (HTTP) and `gpt-live-transcribe` (WebSocket), unchanged code paths.
- Nothing failed, starved or was suspended (`summary.json`).

The other vendors' rows stay in [`../2026-10-19-ci-eu/`](../2026-10-19-ci-eu/) (0.12.0; their code
paths did not change in 0.12.1 or 0.12.2 except the transport defaults, which that run measured).
