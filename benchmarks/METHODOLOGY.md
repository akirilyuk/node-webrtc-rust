# Vendor latency benchmark: methodology

This page says what the numbers in `benchmarks/results/` measure, where each clock starts and
stops, and what the numbers leave out. File and function names point at the code that defines
each timestamp, so a number can be checked against the source. For a quick start see
[`README.md`](README.md).

## What is measured and from where

The bench is the vendor crates of this library (`crates/vendor-<id>`) talking to the vendor APIs
directly. There is no VoiceThere platform hop, no WebRTC transport, no Opus encoder and no jitter
buffer between the vendor and the clock. The numbers are vendor latency as seen by a Rust client
on the machine that ran the bench, not what a caller on a phone hears.

The CLI (`benchmarks/scripts/bench-vendors.mjs`) runs one cargo test per vendor,
`crates/vendor-<id>/tests/live_bench.rs`, with `--nocapture --test-threads=1`. Vendors run one
after another in a single process on the same host, never in parallel, so they do not compete for
CPU or bandwidth. Each sample is printed as one `vendor_bench {json}` line
(`crates/vendor-live-support/src/bench.rs:emit`) and collected into `raw.jsonl`.

## Inputs

**TTS texts.** Two fixed texts (`SHORT` and `SENTENCE` in
`crates/vendor-live-support/src/bench.rs`), recorded as `text` = `short` and `sentence`:

- `SHORT`: "Sure, one moment." (3 words)
- `SENTENCE`: "Thanks for calling, I can help you check the status of your order right now." (15
  words)

**STT phrase.** `STT_PHRASE` in `crates/vendor-live-support/src/audio.rs`: "The quick brown fox
jumps over the lazy dog." (9 words, `STT_WORDS`). Recorded as `text` = `phrase`.

**STT clip.** Each vendor with its own TTS synthesises the phrase once per bench with its own
default TTS (transport `auto`, no voice set, so the crate resolves its default), and the clip is
1000 ms of digital silence, that speech, then `STT_CLIP_TAIL_MS` (3000 ms) of digital silence
(the `live_bench.rs` of each vendor, for example `crates/vendor-deepgram/tests/live_bench.rs`).
Google builds it from its REST TTS voice, or from its gRPC voice on the gRPC leg. AssemblyAI has
no TTS, so it uses the committed fixture `stt_fixture_stereo_48k` in
`crates/vendor-live-support/src/lib.rs`: the same phrase rendered by a local Sherpa Piper voice,
with the same 1000 ms lead and 3000 ms tail. A clip is therefore synthetic speech, not a human
voice, and vendors with their own TTS hear their own voice.

**Pacing.** The clip is streamed in real time. `run_utterance` in
`crates/vendor-live-support/src/stt.rs` sends 20 ms frames of 16 kHz mono s16le, frame n due at
`anchor + n x 20 ms`. A push that finishes more than 100 ms late re-anchors the schedule instead
of sending a burst. The TTS legs have no pacing: one call per sample.

**Peak normalisation and VAD gate.** Before streaming, `run_utterance` scales the clip so its peak
is at -1 dBFS (`normalize_peak_stereo_48k`), because vendor TTS can be quieter than the energy
VAD threshold. A voice activity detector then decides which frames are sent, like the voice agent
does with `gateStt`: the pre-roll before the speech, the speech, and the gate hold after it
(`vad_gated_frames` in `crates/vendor-live-support/src/vad_gate.rs`). The VAD settings are
`runner_vad_config` in `crates/vendor-live-support/src/runner_config.rs`, copied from a VoiceThere
agent runner session log of 2026-10-19 (nwr 0.11.6):

| Setting                   | Value  |
| ------------------------- | ------ |
| `provider`                | energy |
| `threshold`               | 0.05   |
| `min_speech_duration_ms`  | 200    |
| `min_silence_duration_ms` | 1100   |
| `speech_pad_ms`           | 500    |
| `stt_gate_hold_ms`        | 400    |

The pre-roll is `speech_pad_ms + min_speech_duration_ms` (700 ms, `stt_pre_roll_capacity_ms`). The
silence after the speech is never sent in full: the gate closes 1100 ms after the last speech
plus 400 ms hold, and the rest of the 3000 ms tail stays unsent (`pushed_ms` against `total_ms`).

**Models and voices.** Each vendor runs with the defaults of the voice catalog
(`default_tts_model`, `default_stt_model`, `default_tts_voice` of
`packages/voice-catalog`; the data is in `packages/voice-catalog/catalog/vendors.json`). No
`live_bench.rs` sets a TTS voice except Cartesia (`CARTESIA_VOICE_ID` from the environment) and
Google (the catalog default voice, which is put in the record's `model` field). Where the catalog
has no default, the record says `default`. OpenAI's realtime STT model is
`documented_realtime_model` in `crates/vendor-openai`.

## Cold and warm

The definitions are in `crates/vendor-live-support/src/bench.rs`.

**TTS** (`bench_tts`). For each run and for each of the two texts the bench creates a new
provider (`make_provider()`), takes a `cold` sample, then a `warm` sample of the same text on the
same provider object.

- A `cold` sample is the first synthesis of a new provider. It includes whatever the vendor
  crate does on a first call: DNS lookup (unless the operating system answers from its cache),
  TCP connect, TLS handshake, and for WebSocket transports the upgrade. HTTP clients are one per
  provider and WebSocket TTS sockets are kept per provider (for example the "WebSocket reuse
  across utterances" sections of the Deepgram and Cartesia crate READMEs), so a new provider
  starts without a connection.
- A `warm` sample runs on the same provider after the cold one. It reuses the pooled HTTP
  connection or the open socket if the vendor crate keeps one. A vendor that closes its
  connection after every utterance shows a warm number close to its cold one. `http_version`
  tells which protocol the last request used.

**STT, cold only** (`bench_stt`). Each run uses a fresh provider and one utterance; every sample
is `cold`, and `open_ms` is part of it. Vendors measured this way:
Deepgram, ElevenLabs, Fish Audio, AWS, OpenAI realtime (`websocket`), Google gRPC.

**STT, cold and warm** (`bench_stt_warm`). Each run opens a fresh provider (`open_stt`, its time
becomes `open_ms` of the `cold` sample), streams the clip, finalizes, then streams the clip again
on the same open provider; that second utterance is the `warm` sample and has `open_ms` = 0.
Vendors measured this way: AssemblyAI (WebSocket session), Azure (REST), Groq (REST), Google
REST, OpenAI `whisper-1` (`http`). For the REST vendors there is no session to keep, so warm
shows what a reused pooled HTTP connection saves.

Whether a vendor takes a second utterance after a finalize differs per vendor, so only crates
that call `bench_stt_warm` are measured that way.

## Metric reference

Every number is a `metrics` field of a record in `raw.jsonl`. `summary.json` and `summary.md`
keep the numeric ones; `first_before_return` is a boolean and only appears in `raw.jsonl`. TTS
timestamps come from `measure_tts` in `crates/vendor-live-support/src/tts.rs`; STT timestamps
from `run_utterance` and `open_stt` in `crates/vendor-live-support/src/stt.rs`. All clocks are
monotonic (`std::time::Instant` / tokio `Instant`).

### TTS metrics

| Metric                | Unit             | Starts                                                                                                                                                                   | Ends                                                                                                                                                                                                                                                               | Includes                                                                                                                                                                                                                                                       | Does not include                                           |
| --------------------- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| `ttfa_ms`             | ms               | `measure_tts`: `Instant::now()` taken just before the sink receiver task is spawned and `synthesize_progressive` is called, so it starts before the provider is invoked. | The sink receiver task in `measure_tts` stamps `started.elapsed()` when it takes the first chunk off the sink channel. It is the time a chunk is received by the task, not the time the vendor sent it. If no chunk reaches the sink, `ttfa_ms` equals `total_ms`. | On `cold`: connection setup. Request send, vendor processing to first audio, the vendor crate's decode and conversion to 48 kHz stereo s16le (chunks reach the sink already converted, see `assert_chunks_match_source`), and task scheduling in this process. | Network to a listener, jitter buffer, Opus encode, WebRTC. |
| `total_ms` (tts)      | ms               | Same instant as `ttfa_ms`.                                                                                                                                               | `synthesize_progressive` returned (`returned_ms`, taken before the receiver task is joined).                                                                                                                                                                       | The whole synthesis as the caller sees it, including whatever the crate does after the last chunk (building the returned chunks, closing a request).                                                                                                           | Draining the receiver task after the call.                 |
| `audio_ms`            | ms               | n/a                                                                                                                                                                      | n/a                                                                                                                                                                                                                                                                | Sum of the `duration_ms` that the vendor crate declared on the chunks that reached the sink. It is a duration of audio, not a clock.                                                                                                                           | Audio in the returned chunks that never reached the sink.  |
| `chunks`              | count            | n/a                                                                                                                                                                      | n/a                                                                                                                                                                                                                                                                | Chunks received on the sink.                                                                                                                                                                                                                                   | n/a                                                        |
| `stall_ms`            | ms               | The arrival of the first sink chunk.                                                                                                                                     | The arrival of each later chunk.                                                                                                                                                                                                                                   | Worst lateness of a chunk against gapless playback (`stall_ms` function): max over k of `max(0, (arrival_k - arrival_0) - sum of the durations before k)`. Above 0 means an audible gap in a player that starts at the first chunk.                            | Buffering a real player would add.                         |
| `bytes`               | bytes            | n/a                                                                                                                                                                      | n/a                                                                                                                                                                                                                                                                | PCM bytes that reached the sink (48 kHz stereo s16le, 192 bytes per ms).                                                                                                                                                                                       | Bytes only present in the return value.                    |
| `returned_bytes`      | bytes            | n/a                                                                                                                                                                      | n/a                                                                                                                                                                                                                                                                | PCM bytes in the chunks the call returned. Equal to `bytes` for a vendor that sends each chunk once.                                                                                                                                                           | n/a                                                        |
| `first_before_return` | boolean          | n/a                                                                                                                                                                      | n/a                                                                                                                                                                                                                                                                | True when the first sink chunk arrived before the call returned, meaning the vendor streamed. `raw.jsonl` only.                                                                                                                                                | n/a                                                        |
| `rms`                 | s16 sample units | n/a                                                                                                                                                                      | n/a                                                                                                                                                                                                                                                                | RMS of the sink PCM (`audio::rms`). A near-zero value means silence came back.                                                                                                                                                                                 | n/a                                                        |
| `real_time_factor`    | ratio            | n/a                                                                                                                                                                      | n/a                                                                                                                                                                                                                                                                | Derived, not emitted by Rust: `total_ms / audio_ms` (`withDerivedMetrics` in `scripts/ci/vendor-bench-summary.mjs`), only for TTS records with `audio_ms` above 0. Below 1 is faster than real time.                                                           | n/a                                                        |

### STT metrics

| Metric                     | Unit  | Starts                                                                                                                                                                                  | Ends                                                                                                                                                                                         | Includes                                                                                                                                                                                                                                                                                                                                                                 | Does not include                                                                                                                                                         |
| -------------------------- | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `open_ms`                  | ms    | `open_stt`: just before `start()`.                                                                                                                                                      | `wait_ready` returned true (each call bounded at 10 s).                                                                                                                                      | Connect, handshake and session setup as far as the vendor crate's `wait_ready` waits. For a crate that keeps the default `wait_ready` (always ready), only `start()`.                                                                                                                                                                                                    | The `warm` sample of `bench_stt_warm`, where it is 0 because the session is already open.                                                                                |
| `first_partial_ms`         | ms    | The moment the first voiced frame is about to be pushed (`speech_start_at`). A voiced frame is a 20 ms frame with at least one non-zero sample, so the pre-roll silence does not count. | The first `Partial` transcript seen by the collector before the finalize. Transcripts are only seen after a push or poll returns, so the resolution is one frame (20 ms) plus the push time. | Vendor time to a first partial, including the real-time pacing delay of the audio itself.                                                                                                                                                                                                                                                                                | `null` when no partial arrived before the finalize (finals-only vendors); `null` is left out of the statistics, so `n` of this metric can be lower than the group's `n`. |
| `speech_end_to_final_ms`   | ms    | The moment the last voiced frame is about to be pushed (`speech_end_at`).                                                                                                               | The moment the collector saw the last `Final` of the utterance (`last_final_at`).                                                                                                            | The silent frames the gate still sends after the last voiced frame (the VAD waits `min_silence_duration_ms` = 1100 ms for the end of speech, then the gate holds `stt_gate_hold_ms` = 400 ms), then the finalize request, vendor processing, and for batch vendors the upload. A vendor that sends its final on its own before the finalize can come in below that wait. | Time spent before the last voiced frame.                                                                                                                                 |
| `finalize_to_final_ms`     | ms    | Just before `request_finalize` (`Deferred` providers) or `finalize_utterance` (`Inline` providers).                                                                                     | Same `last_final_at`.                                                                                                                                                                        | The part of the wait the vendor owns after end of speech was declared by the gate. 0 when the final arrived before the finalize was requested. For `Inline` providers the final is only observed after `finalize_utterance` returns, so this includes that whole call.                                                                                                   | The VAD end-of-speech wait.                                                                                                                                              |
| `partials_before_finalize` | count | n/a                                                                                                                                                                                     | n/a                                                                                                                                                                                          | `Partial` transcripts seen before the finalize was requested.                                                                                                                                                                                                                                                                                                            | n/a                                                                                                                                                                      |
| `matched_words`            | count | n/a                                                                                                                                                                                     | n/a                                                                                                                                                                                          | How many of the 9 expected words occur in the concatenated transcript (`audio::matched_words`). A sanity check on recognition; the transcript is not printed or stored. The bench does not fail on a low count.                                                                                                                                                          | n/a                                                                                                                                                                      |
| `finals`                   | count | n/a                                                                                                                                                                                     | n/a                                                                                                                                                                                          | `Final` transcripts of the one utterance. The library's contract is exactly 1.                                                                                                                                                                                                                                                                                           | n/a                                                                                                                                                                      |
| `pushed_ms`                | ms    | n/a                                                                                                                                                                                     | n/a                                                                                                                                                                                          | Audio the VAD gate sent to the vendor.                                                                                                                                                                                                                                                                                                                                   | The unsent tail.                                                                                                                                                         |
| `total_ms` (stt)           | ms    | n/a                                                                                                                                                                                     | n/a                                                                                                                                                                                          | Length of the whole clip: 1000 ms lead, speech, 3000 ms tail. Not a clock.                                                                                                                                                                                                                                                                                               | n/a                                                                                                                                                                      |

### Record fields

Each `raw.jsonl` line is a `BenchRecord` (`bench.rs`). The grouping fields are the first six.

| Field          | Meaning                                                                                                                                                                                                                              |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `vendor`       | Vendor id, same as the crate suffix: `deepgram`, `elevenlabs`, `cartesia`, `assemblyai`, `google`, `groq`, `azure`, `aws`, `openai`, `fishaudio`.                                                                                    |
| `leg`          | `tts` or `stt`.                                                                                                                                                                                                                      |
| `transport`    | TTS: each transport the catalog lists for the vendor (`tts_transports`), such as `websocket` or `http`. STT: a fixed label per bench (`websocket`, `http`, `rest`, `grpc`).                                                          |
| `model`        | The catalog default model that ran, `default` where the catalog has none, and the voice for Google TTS.                                                                                                                              |
| `phase`        | `cold` or `warm`, as defined above.                                                                                                                                                                                                  |
| `text`         | `short`, `sentence` (TTS) or `phrase` (STT).                                                                                                                                                                                         |
| `run`          | Run index, starting at 0.                                                                                                                                                                                                            |
| `http_version` | `last_transport()` of the provider after the sample: `h2`, `http/1.1`, `ws` or `grpc` where the vendor crate reports it, otherwise `null`. For `bench_stt_warm` the cold and warm sample share one value read after both utterances. |
| `metrics`      | The sample object whose fields are the tables above.                                                                                                                                                                                 |

## Statistics

`scripts/ci/vendor-bench-summary.mjs` groups records by `vendor`, `leg`, `transport`, `model`,
`phase` and `text`, and computes per numeric metric:

- `median`: the middle value, or the mean of the two middle values for an even count
  (`median`).
- `p90`: nearest rank, the value at index `ceil(0.9 x n) - 1` of the sorted samples
  (`percentile`). With 5 runs that is the maximum, and with 3 runs too. Use at least 10 runs
  before reading p90 as a percentile.
- `min`, `max`, and `n`, the number of samples that had that metric.

Values that are not finite numbers, such as `null` or booleans, are skipped. A group also lists
the sorted `http_versions` seen. The CLI default is 5 runs (`--runs`).

## Environment record

`environment.json` is written next to the results by `collectEnvironment` in
`benchmarks/scripts/environment.mjs`, so a result can be read together with where it ran.

| Field           | Meaning                                                                                    |
| --------------- | ------------------------------------------------------------------------------------------ |
| `nwr_version`   | `version` in `packages/sdk/package.json` of the checkout that ran, `null` if unreadable.   |
| `git_sha`       | `git rev-parse HEAD` of the checkout.                                                      |
| `git_dirty`     | True when `git status --porcelain` is not empty, so local changes were built into the run. |
| `os`            | `platform`, `release` and `arch` from Node's `os` module.                                  |
| `cpu_model`     | Model name of the first CPU.                                                               |
| `cpu_cores`     | Number of logical CPUs reported by `os.cpus()`.                                            |
| `memory_gb`     | Total memory in GB, one decimal.                                                           |
| `node_version`  | `process.version` of the CLI.                                                              |
| `rustc_version` | `rustc --version`, `null` if not found.                                                    |
| `region`        | The `--region` label (default `unlabelled`). It is a label you choose, nothing detects it. |
| `started_at`    | ISO time when the CLI started its first vendor.                                            |
| `finished_at`   | ISO time after the last vendor finished.                                                   |
| `endpoints`     | One entry per probed vendor host (below). Empty when probes are switched off.              |

**Endpoint probes.** After all vendors ran, the CLI probes the hosts the vendor crates connect
to (`hosts` in `benchmarks/scripts/vendors.mjs`, checked against the crate sources by
`vendors.test.mjs`) for the vendors that ran. Per host, three samples of each: DNS lookup
(`dns.lookup`, which goes through the operating system resolver, so a cached answer reads near
zero), TCP connect to port 443, and a TLS handshake offering `h2` and `http/1.1`. The median
is recorded. No HTTP request is sent. Each probe is bounded at 5 s. An entry has these fields:

| Field            | Meaning                                                                                                                               |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `vendor`         | Vendor id.                                                                                                                            |
| `host`           | Probed host name. Hosts with a `{region}` part are only probed when the region variable (`AZURE_SPEECH_REGION`, `AWS_REGION`) is set. |
| `dns_ms`         | Median DNS time, ms.                                                                                                                  |
| `tcp_ms`         | Median TCP connect time, ms.                                                                                                          |
| `tls_ms`         | Median time to a finished TLS handshake, ms. Includes the TCP connect.                                                                |
| `alpn`           | Protocol the server picked (`h2`, `http/1.1`), `null` if none.                                                                        |
| `address_family` | `IPv4` or `IPv6`, from the DNS answer used.                                                                                           |
| `error`          | Only present when a probe failed or timed out: the error code of the first failure. A failed probe leaves its field `null`.           |

The probes run from the Node CLI after the benchmark, not from the Rust process during it. They
show the path to the vendor from this host, not the exact path of each sample.

**`summary.json`.** The top-level fields of the file the compare tool reads:

| Field             | Meaning                                                                                                                                                   |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `schema_version`  | Format version of this file, currently 1.                                                                                                                 |
| `generated_at`    | ISO time the CLI finished (same as `finished_at`).                                                                                                        |
| `region`          | The `--region` label.                                                                                                                                     |
| `runs`            | Runs per sample group, from `--runs`.                                                                                                                     |
| `transport`       | The `--transport` value passed on to the benches (`all` by default).                                                                                      |
| `vendors_run`     | Vendors whose run step started, including a vendor whose run failed.                                                                                      |
| `skipped`         | `{ vendor, reason }` entries: `excluded` or `no key: <variable names>`.                                                                                   |
| `failed`          | Entries with `vendor`, `phase` (`env`, `build` or `run`), and `exit_code`, `log` or `reason` where known. A failure makes the CLI exit with 1.            |
| `starved`         | Vendors whose run window was CPU-starved (below).                                                                                                         |
| `host_suspended`  | `{ vendor, at, gap_ms }` for each suspend gap (below).                                                                                                    |
| `per_vendor_load` | Per vendor id: `samples`, `busy_median`, `busy_p90`, `busy_max` (CPU busy percent per second), `load1_max` (largest 1-minute load average) and `starved`. |
| `groups`          | The statistics of the previous section, one entry per group with its `n`, `http_versions` and `metrics`.                                                  |

**Host load.** While a vendor's test runs, the CLI samples the busy share of `os.cpus()` every
second. The cargo build happens before and is not sampled. A vendor window is `starved` when the
busy p90 is above 85 %. Inside a container `os.cpus()` reports the CPUs of the node, not the
container's CPU quota, so a starved container on a busy shared CI node can go unnoticed.

**Host suspend.** A timer ticks every second during each vendor's run. A gap above 5 s between
two ticks means the host was suspended (a sleeping laptop). The run is then marked
`[host-suspended]`, `result: inconclusive`, and is repeated on a host that stays awake rather than
counted or discarded.

## What the numbers do not tell you

- Network and vendor processing are not separated. The endpoint probes give a rough size of the
  network part (DNS, TCP and TLS round trips), nothing more.
- One results folder is one region and one point in time. Vendor load varies with the time of day
  and day of the week, so differences of a few tens of ms between two single runs are noise.
- Our own pipeline cost is inside the sink timing. Chunks arrive at the sink after the vendor
  crate decoded and converted them to 48 kHz stereo, so a vendor whose audio needs more
  conversion or whose chunks are small pays for that in `ttfa_ms`. The bench does not time the
  conversion alone.
- Streaming and batch STT are not the same kind of number. For a vendor that uploads the whole
  utterance at the finalize (Azure, Groq, Google REST, OpenAI `whisper-1`), `speech_end_to_final_ms`
  includes the upload and a full-file recognition, and it has no `first_partial_ms`.
- The speech is synthetic, one short English phrase. Recognition speed on natural, noisy or long
  speech can differ.
- `speech_end_to_final_ms` contains the VAD end-of-speech wait of the runner settings. Compare it
  across vendors on the same settings, not with an end-pointing number from another tool.

## Known vendor behaviours

Observed in our runs, 2026-10 (single runs made before this bench existed, not yet repeated
with it):

- ElevenLabs STT: the first partial arrived 1.6 to 2.0 s after the first audio.
- Deepgram TTS over WebSocket: 520 to 919 ms to first audio on a new socket, 108 ms on a reused
  socket.

From the vendor crates (`emits_partials()` returns false in `crates/vendor-<id>/src/stt.rs`),
these STT paths send only finals: Azure (REST short audio), Fish Audio (`transcribe-1` realtime
socket), Groq (file upload), Google REST `latest_long`, and OpenAI `whisper-1` over `http`. For
them `first_partial_ms` is `null` and `partials_before_finalize` is 0. Of these, Azure, Groq,
Google REST and `whisper-1` upload the finished utterance as a file or request body at the
finalize (their crate READMEs and `live_bench.rs` comments); Fish Audio streams audio and
commits by hand, but still reports no partials.

## Reproduce

Prerequisites: a Rust toolchain, Node 18 or newer, a vendor key. From the repository root:

```bash
npm run bench:vendors -- --vendors deepgram,openai --runs 5 --region my-host --env-file <path>
```

Flags:

- `--vendors <list|all>` and `--exclude <list>` choose vendors. A vendor without keys is skipped;
  with `--require` it is a failure instead.
- `--runs <n>` runs per group (default 5).
- `--transport <all|websocket|http|auto|rest|grpc>` or a comma list selects transports.
- `--region <label>` names where you run (used in the output folder name).
- `--out <dir>` overrides `benchmarks/results/<date>-<region>/`; an existing `summary.json` there
  needs `--force`.
- `--env-file <path>` reads whitespace-separated `NAME=value` tokens in addition to the process
  environment. A key value is never printed or written to a result file.

Variables and hosts per vendor:

| Vendor       | Variables                                                                                      | Probed hosts                                                                 |
| ------------ | ---------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `deepgram`   | `DEEPGRAM_API_KEY`                                                                             | `api.deepgram.com`                                                           |
| `elevenlabs` | `ELEVENLABS_API_KEY`                                                                           | `api.elevenlabs.io`                                                          |
| `cartesia`   | `CARTESIA_API_KEY`, `CARTESIA_VOICE_ID`                                                        | `api.cartesia.ai`                                                            |
| `assemblyai` | `ASSEMBLYAI_API_KEY`                                                                           | `streaming.assemblyai.com`                                                   |
| `google`     | `GOOGLE_API_KEY` (REST), or `GOOGLE_APPLICATION_CREDENTIALS` and `GOOGLE_CLOUD_PROJECT` (gRPC) | `texttospeech.googleapis.com`, `speech.googleapis.com`                       |
| `groq`       | `GROQ_API_KEY`                                                                                 | `api.groq.com`                                                               |
| `azure`      | `AZURE_SPEECH_KEY`, `AZURE_SPEECH_REGION`                                                      | `<region>.tts.speech.microsoft.com`, `<region>.api.cognitive.microsoft.com`  |
| `aws`        | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`                                     | `polly.<region>.amazonaws.com`, `transcribestreaming.<region>.amazonaws.com` |
| `openai`     | `OPENAI_API_KEY`                                                                               | `api.openai.com`                                                             |
| `fishaudio`  | `FISH_API_KEY`                                                                                 | `api.fish.audio`                                                             |

To compare your machine with a published run:

```bash
npm run bench:compare -- benchmarks/results/<ours> benchmarks/results/<yours>
```

It prints the region, `nwr_version` and CPU of both folders, then one row per group and metric
present in both: the two medians, the delta and the delta in percent of the first folder. Groups
in only one folder are listed after the table. Read a delta together with the environment: another
region, another `nwr_version` or a `starved` entry explains more than the vendor does. Different
`runs` change how stable the medians are, not what they mean.

Google, Azure and AWS are supported by the CLI but are not in our published results yet: not
measured yet.

## Refresh policy

Results are refreshed for every minor nwr release and on demand. A results folder is dated and is
never edited after it is committed. A failed vendor run is investigated and its cause written
down; it is not re-run until the numbers look better.
