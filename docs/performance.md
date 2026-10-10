# Performance

This page covers the library's offline speech engines (Sherpa-ONNX streaming speech-to-text and Piper/Melo text-to-speech) and the media path around them. Everything was measured in a CPU-limited container, so the numbers are per CPU and scale with the CPUs you give a process. Every table can be reproduced with the commands in [Reproduce](#reproduce).

## Speech-to-text (Sherpa-ONNX streaming)

Streaming recognition runs one decode per audio chunk per stream. The table lists, for each number of concurrent streams on one CPU, how far decoding falls behind live audio (lag), how long the final transcript takes after the speaker stops (final), and how much faster than real time the engine decodes.

<!-- MEASURE:stt-capacity -->

One ONNX thread per CPU works best for streaming STT. Two threads on one CPU compete with each other and add scheduling overhead without finishing a decode sooner. `SHERPA_POOL_MAX_CONCURRENT_DECODE` limits how many decodes run at the same time, so extra streams wait for a slot instead of oversubscribing the CPUs.

## Text-to-speech cost per voice

Synthesis cost is reported as a real-time factor (RTF): CPU seconds spent per second of audio produced. An RTF of 0.2 means one CPU can synthesize five seconds of speech per second. The table also shows each voice's cost relative to `amy-medium`, and the time to first audio when 1, 2 and 4 requests arrive at once.

<!-- MEASURE:tts-voice-cost -->

The `-low` and `-medium` Piper voices cost about the same. The `-high` voices and Melo (`vits-melo-tts-zh_en`) cost several times more per audio second, so plan CPUs by voice, not by request count.

## Threads and CPUs for expensive voices

<!-- MEASURE:tts-threads -->

`SHERPA_TTS_NUM_THREADS` sets the ONNX intra-op threads of one synthesis. Giving the process more CPUs without raising the thread count does not speed up a single synthesis, because one synthesis only uses the threads it was configured with. Setting threads equal to CPUs shortens the time to first audio, and the CPU spent per audio second stays about the same.

## Engines and memory

`SHERPA_POOL_MAX_CONCURRENT_TTS` sets how many syntheses run in parallel. On one CPU a second engine does not lower latency, since the two syntheses share the same CPU. Each engine also loads its own copy of the model, so memory grows with the engine count.

<!-- MEASURE:tts-engine-rss -->

## Phrase cache

When the same text is spoken again with the same voice settings, the audio comes from an in-memory cache and no synthesis runs. `VOICE_TTS_PHRASE_CACHE` controls it (on by default; `0`, `false`, `no` or `off` disables it), and `VOICE_TTS_PHRASE_CACHE_MAX_BYTES` and `VOICE_TTS_PHRASE_CACHE_MAX_ENTRY_BYTES` bound its size. The OpenTelemetry counters `sherpa_tts_phrase_cache_hits` and `sherpa_tts_phrase_cache_misses` show how often it helps.

A synthesis that was cancelled part way (for example by a barge-in) is never cached, so a later request for the same text gets the full phrase. This is covered by [`tts_cancel_not_cached_test.rs`](../crates/vendor-sherpa-onnx/tests/tts_cancel_not_cached_test.rs).

## Media path

These are micro benchmarks of the code that runs for every audio frame. They run through [`scripts/perf/run-perf-baseline.sh`](../scripts/perf/run-perf-baseline.sh).

<!-- MEASURE:media-benches -->

| Bench           | Kind      | What it measures                                                                                                                                                     |
| --------------- | --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `inbound_frame` | criterion | Cost of one `VoiceAgent::process_inbound_pcm` call with a mock STT, energy VAD and an open gate, for a silent and a voiced frame, plus allocations per voiced frame. |
| `resample`      | criterion | Resampling 10 s of 22,050 Hz mono audio to 48 kHz stereo s16le, one-shot and streaming (0.5 s pushes), plus peak heap use.                                           |
| `opus_encode`   | criterion | `PcmEncoder::encode` on one 20 ms stereo frame with the default Opus settings.                                                                                       |
| `tts_drain`     | probe     | CPU cost per 20 ms frame of draining a 10 s TTS chunk into a writer, and bytes allocated per frame as a proxy for copies.                                            |
| `idle_agents`   | probe     | CPU time per second per agent for 100 started `VoiceAgent`s that are idle for 5 s.                                                                                   |

## Voice loopback under load

The `start:roundtrip-load` example runs several speaker/listener legs at the same time in one process. Each leg is a WebRTC loopback with a Sherpa TTS speaker and a Sherpa STT listener, and each leg plays several turns in sequence. A run fails if any turn does not produce a `user_speech_final` containing the leg's keyword, or if anything throws or times out.

It has two modes. The normal mode runs `SHERPA_LOAD_LEGS` legs (default 8) for `SHERPA_LOAD_TURNS` turns (default 5). Capacity mode (`SHERPA_LOAD_CAPACITY=1`) steps through 2, 4, 6, 8, 10, 12, 16, 20, 24, 32, 40 and 48 legs until a step fails. A step passes while it has no failures and its final-latency p95 stays within `SHERPA_LOAD_SLO_DELTA_MS` (default 1000) of the 2-leg step.

The report has final-latency percentiles, CPU per session-minute, event-loop lag and RSS. These are reported, not gated. The npm scripts turn the phrase cache off (`VOICE_TTS_PHRASE_CACHE=0`) so every turn runs a real synthesis; `start:roundtrip-load-cached` keeps it on.

<!-- MEASURE:roundtrip-load -->

## Reliability under load

These tests cover the failure modes that show up when many sessions start, stop and share engines.

- Concurrent STT decodes give the same transcript as serial decodes: [`stt_concurrent_equals_serial_test.rs`](../crates/vendor-sherpa-onnx/tests/stt_concurrent_equals_serial_test.rs)
- Starting and stopping sessions repeatedly releases the agent's workers and audio writer, including when a session is dropped without `stop()` and when it is dropped mid-playback: [`session_churn_test.rs`](../crates/speech/tests/session_churn_test.rs)
- Pool churn: after many STT and TTS sessions the pool's active-session count returns to zero: [`session_churn_pool_test.rs`](../crates/vendor-sherpa-onnx/tests/session_churn_pool_test.rs)
- A cluster speech client dropped without `stop()` closes its streams: `cluster_stt_drop_without_stop_closes_streams` in [`cluster_vendor_test.rs`](../crates/vendor-cluster-speech/tests/cluster_vendor_test.rs)
- Large messages (1 MiB and 4 MB single TTS messages arrive intact, and messages over the default gRPC limit are accepted): `tts_receives_1mib_single_message_intact`, `tts_receives_4mb_single_message_intact` and `tts_accepts_message_over_default_limit` in [`cluster_vendor_test.rs`](../crates/vendor-cluster-speech/tests/cluster_vendor_test.rs)
- A cancelled TTS synthesis is never cached: [`tts_cancel_not_cached_test.rs`](../crates/vendor-sherpa-onnx/tests/tts_cancel_not_cached_test.rs)
- Lexicon models (Melo) synthesize, and Piper models still do: [`tts_lexicon_model_test.rs`](../crates/vendor-sherpa-onnx/tests/tts_lexicon_model_test.rs)
- Model folders that contain only git-lfs pointer files are rejected with a clear error instead of aborting ONNX Runtime, and Melo folders resolve their lexicon, dictionary and rule FSTs: unit tests in [`tts_model_paths.rs`](../crates/vendor-sherpa-onnx/src/tts_model_paths.rs)

## Tuning

| Variable                            | Default               | What it changes                                                                                                                                     | Recommendation                                                           |
| ----------------------------------- | --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| `SHERPA_STT_NUM_THREADS`            | `1`                   | ONNX intra-op threads of the streaming recognizer.                                                                                                  | 1 thread per CPU.                                                        |
| `SHERPA_POOL_MAX_CONCURRENT_DECODE` | CPU count (minimum 1) | How many STT decodes run at once.                                                                                                                   | Leave at the CPU count.                                                  |
| `SHERPA_TTS_NUM_THREADS`            | `2`                   | ONNX intra-op threads of one synthesis.                                                                                                             | `-low` / `-medium` voices: 1. `-high` voices and Melo: threads = CPUs.   |
| `SHERPA_POOL_MAX_CONCURRENT_TTS`    | `2`                   | How many syntheses run at once, and how many engines (model copies) are loaded.                                                                     | `-low` / `-medium` voices: 1 engine per CPU. `-high` voices and Melo: 1. |
| `VOICE_TTS_PHRASE_CACHE`            | on                    | Serves repeated phrases from memory. Size limits: `VOICE_TTS_PHRASE_CACHE_MAX_BYTES` (16 MiB) and `VOICE_TTS_PHRASE_CACHE_MAX_ENTRY_BYTES` (2 MiB). | Keep on for agents that repeat greetings and prompts.                    |

## Reproduce

Run these from the repository root. The speech probes need the models downloaded first.

Download the models once. The STT model is the example's default English streaming Zipformer, and the TTS voices are the ones the voice-cost probe compares (`npm run download-tts:list --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa` shows every voice):

```bash
npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en-amy-medium --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en-lessac-low --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en-lessac --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en-lessac-high --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:zh --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Media path micro benchmarks (criterion benches and the `tts_drain` and `idle_agents` probes):

```bash
bash scripts/perf/run-perf-baseline.sh <label>
```

TTS voice cost, thread and engine sweeps, and STT stream capacity, limited to one CPU and repeated three times:

```bash
PROBE_MODELS_DIR=examples/voice-agent-local-sherpa/.models bash scripts/perf/run-voice-cost.sh --cpus 1 --runs 3
```

The script drives the `tts_voice_cost_probe` and `stt_stream_capacity_probe` tests in `crates/vendor-sherpa-onnx/tests/voice_cost_probe.rs` and summarizes them with `scripts/perf/voice-cost-report.mjs`.

Voice loopback under load, in normal mode and in capacity mode:

```bash
npm run start:roundtrip-load --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
SHERPA_LOAD_CAPACITY=1 SHERPA_LOAD_TURNS=2 npm run start:roundtrip-load --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```
