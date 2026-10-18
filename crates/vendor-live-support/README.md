# node-webrtc-rust-vendor-live-support

Shared test support for the live vendor tests and the latency bench. `publish = false`; each
vendor crate lists it under `[dev-dependencies]` only. Nothing here prints a key or a transcript.

## What it holds

| Module     | Purpose                                                                                                                                                  |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `vad_gate` | Gates a 48 kHz stereo clip with the voice agent's default energy VAD, so only speech (plus 300 ms pre-roll) is streamed to STT vendors.                  |
| `audio`    | `rms`, `matched_words`, `STT_PHRASE` / `STT_WORDS`.                                                                                                      |
| `env`      | `live_env(vendor_id, vars)`: a missing variable skips locally and panics (naming the variable, never a value) under `<VENDOR_ID>_LIVE_REQUIRED=1`.       |
| `tts`      | `measure_tts` and `assert_tts_contract`: at least 2 chunks, first chunk before the call returns, sink bytes equal returned bytes, sane length and level. |
| `stt`      | `measure_stt` and `assert_stt_contract`: anchor-paced push, Deferred or Inline finalize, every wait bounded, `emits_partials()` equals partials seen.    |
| `bench`    | `BenchRecord`, `emit` (one `vendor_bench <json>` line), `bench_tts`, `bench_stt`.                                                                        |

STT audio is exactly what the voice agent pushes to a vendor: 20 ms frames (640 bytes) of
16 kHz mono s16le, the downmix of the 48 kHz stereo WebRTC frame. TTS chunks are 48 kHz stereo
s16le with their own `duration_ms`.

## Run a live test locally

```bash
# from node-webrtc-rust/
FISH_API_KEY=... cargo test -p node-webrtc-rust-vendor-fishaudio --features live --test live_test -- --nocapture --test-threads=1
```

Without the key the tests print `<id> live test skipped: <VARS> unset` and pass. With
`<ID>_LIVE_REQUIRED=1` (CI sets it) they panic instead, so a skipped run can never show green.
Use `--test-threads=1`: vendors cap concurrent requests and the tests print timing lines
(`<id>_ttfa_ms`, `<id>_stt_final_ms`, `<id>_stt_pushed_ms`) that CI copies into the step summary.

CI: `.github/workflows/vendor-live-<id>.yml`. `tests/workflow_lockstep_test.rs` fails the PR when a
`crates/vendor-*/tests/live_test.rs` has no such workflow, or the workflow lacks the fork-skip
`if`, `_LIVE_REQUIRED: '1'`, `--test-threads=1` or the path filter.

## Latency bench

`crates/vendor-<id>/tests/live_bench.rs` calls `bench_tts` / `bench_stt` and is dispatched with the
workflow `.github/workflows/vendor-bench.yml` (inputs `vendor`, `runs`; no pull request trigger).
Locally:

```bash
NWR_BENCH_RUNS=3 DEEPGRAM_API_KEY=... DEEPGRAM_LIVE_REQUIRED=1 \
  cargo test -p node-webrtc-rust-vendor-deepgram --features live --test live_bench -- --nocapture --test-threads=1 \
  | tee bench.log
node scripts/ci/vendor-bench-summary.mjs bench.log bench.json
```

TTS: per run and per text (`short`, `sentence`) a new provider gives a `cold` sample (DNS and
TLS or WS handshake included) and then a `warm` sample on the same provider. STT: each run is a
fresh provider and one utterance, `cold`. A warm second utterance is not measured because whether
a provider accepts another utterance after a finalize differs per vendor; a vendor crate opts in
with `stt::run_utterance` once its docs allow it.

## STT fixture

`fixtures/quick-brown-fox-16k-mono.pcm` is 16 kHz mono s16le of "The quick brown fox jumps over
the lazy dog." (3.36 s), rendered by local Sherpa `vits-piper-en_US-amy-low`.
`stt_fixture_stereo_48k()` returns it as 1 s of silence, the phrase upsampled to 48 kHz stereo,
and 2 s of silence (longer than the VAD's 1300 ms hangover, so the segment ends).

Regenerate on purpose (Piper noise changes the bytes on every render):

```bash
# from node-webrtc-rust/; uses $SHERPA_TTS_MODEL_PATH, else $NWR_CI_SHERPA_MODELS/vits-piper-en_US-amy-low,
# else examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low
bash crates/vendor-live-support/scripts/make-fixture.sh
```

The script runs the `#[ignore]` test `make_stt_fixture_test` of `node-webrtc-rust-vendor-sherpa-onnx`.
