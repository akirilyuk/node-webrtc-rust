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
The Google, Azure and AWS workflows are additionally gated by the repository variables `GOOGLE_LIVE_ENABLED`, `AZURE_LIVE_ENABLED` and `AWS_LIVE_ENABLED` (skipped until set); no other vendor workflow may carry such a gate.

## Latency bench

The CLI `npm run bench:vendors` (see [`benchmarks/README.md`](../../benchmarks/README.md)) runs these benches for chosen vendors and records the host. Definitions of every metric: [`benchmarks/METHODOLOGY.md`](../../benchmarks/METHODOLOGY.md).

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
TLS or WS handshake included) and then a `warm` sample on the same provider. STT: `bench_stt` uses a
fresh provider and one utterance per run, `cold`. Whether a provider accepts another utterance
after a finalize differs per vendor, so a vendor whose docs allow it uses `bench_stt_warm`
instead: a `cold` utterance (open included) and then a `warm` second utterance on the same
session (AssemblyAI, Azure, Groq, Google REST, OpenAI `whisper-1`).

## STT fixture

`fixtures/quick-brown-fox-16k-mono.pcm` is 16 kHz mono s16le of "The quick brown fox jumps over
the lazy dog." (3.36 s), rendered by local Sherpa `vits-piper-en_US-amy-low`.
`stt_fixture_stereo_48k()` returns it as 1 s of silence, the phrase upsampled to 48 kHz stereo,
and 3 s of silence (`STT_CLIP_TAIL_MS`: the gate ends the segment 1100 ms after the speech and holds 400 ms more, so at least 1 s is not streamed).
The gate (`vad_gate::gate_vad_config`) is the runner's VAD config from `runner_config::runner_vad_config` (threshold 0.05, minimum silence 1100 ms, hold 400 ms, pad 500 ms); a unit test pins the two together.

Regenerate on purpose (Piper noise changes the bytes on every render):

```bash
# from node-webrtc-rust/; uses $SHERPA_TTS_MODEL_PATH, else $NWR_CI_SHERPA_MODELS/vits-piper-en_US-amy-low,
# else examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low
bash crates/vendor-live-support/scripts/make-fixture.sh
```

The script runs the `#[ignore]` test `make_stt_fixture_test` of `node-webrtc-rust-vendor-sherpa-onnx`.
