# OpenAI TTS with the playout prebuffer, 2026-10-20

A check of the 0.12.2 fix for gaps in OpenAI streamed text-to-speech, not a full benchmark run.

- **What:** OpenAI only (`tts-1` TTS over HTTP, plus the STT rows the bench always runs), 10 runs,
  built from commit `ad5314f` (the fix branch; released in 0.12.2). `environment.json` reports
  0.12.1 because the package version was bumped after this run.
- **Where:** an Apple M4 Pro laptop on a home connection, label `mac-prebuffer`. These numbers are
  not comparable with the EU server runs in [`../2026-10-19-ci-eu/`](../2026-10-19-ci-eu/): the
  network path differs.
- **Result:** no TTS sample stalled (`stall_ms` 0 in all 40). Playback starts at
  `playout_start_ms`, about 230–260 ms after the first chunk arrives (`ttfa_ms`), because the
  crate buffers 250 ms of audio before playing. In the EU run without the buffer, 26 of 40 samples
  stalled.
- **Why:** OpenAI sends a first chunk of about 40 ms of audio and then nothing for up to about 1 s.
  See "Known vendor behaviours" and the `playout_start_ms` / `stall_ms` rows in
  [`METHODOLOGY.md`](../../METHODOLOGY.md).
