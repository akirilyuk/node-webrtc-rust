# Runs behind the 0.10.1 vs 0.11.0 perf comparison (2026-10-15)

Apple M4 Pro, 14 cores, AC power, `caffeinate -s`. Base 0.10.1 (`release/0.10.1`, `a194e23`, release build) and head the 0.11.0 code as of #439 (STT task, idle agents polled once per second; release build). The final 0.11.0 idle change (#441, no polling while idle) was measured separately in the idle-agents table. Every measurement alternated base and head, and the host load was logged before each step. macOS media analysis was busy during part of the evening (load average 4–14 before the steps), which affects base and head alike.

## Media-path micro benchmarks (`scripts/perf/run-perf-baseline.sh`)

`node scripts/perf/compare-perf.mjs base.json head.json` (band: max(10 %, 2 × base spread)):

| Metric                           |    0.10.1 |    0.11.0 |   Delta | Verdict                        |
| -------------------------------- | --------: | --------: | ------: | ------------------------------ |
| idle_agent_cpu_us_per_agent_s    |    107.90 |    123.81 | +14.7 % | over the 10 % band (see below) |
| inbound_frame_voiced_ns          |    2670.2 |    2741.8 |  +2.7 % | ok                             |
| inbound_frame_silence_ns         |    2689.9 |    2645.8 |  −1.6 % | ok                             |
| inbound_frame_allocs             |     7.060 |     7.022 |  −0.5 % | ok                             |
| opus_encode_20ms_ns              |   112 680 |   112 820 |  +0.1 % | ok                             |
| resample_22k_10s_ns              | 1 049 200 | 1 044 800 |  −0.4 % | ok                             |
| resample_stream_22k_10s_ns       |   895 840 |   895 690 |   0.0 % | ok                             |
| tts_drain_frames_ns_per_frame    |    21 448 |    21 493 |  +0.2 % | ok                             |
| tts_drain_bytes_copied_per_frame |    6158.3 |    6159.3 |   0.0 % | ok                             |

Idle agents: the first 0.11.0 build polled the STT provider every 20 ms even when idle and measured 214–239 µs per agent-second against 102–144 on 0.10.1 (3 alternating `idle_agents` runs each). Polling once per second while idle brought it to 111–152 (124 in the comparison run above). The released code does not poll an idle agent at all (it polls every 20 ms from the first audio or finalize request until 10 s after the last one). Five alternating runs of that code (#441, as released in 0.11.0) against 0.10.1, load average 3.8–4.7:

| Run | 0.10.1 | 0.11.0 |
| --: | -----: | -----: |
|   1 |  143.1 |  141.3 |
|   2 |  137.0 |  132.7 |
|   3 |  147.6 |  172.1 |
|   4 |  154.1 |  164.9 |
|   5 |  150.2 |  159.3 |

Median 148 vs 159 µs per idle agent-second; the ranges overlap. The difference (about 0.001 % of one core per idle agent) is within run-to-run noise on this host. The other micro benchmarks were measured on the #439 code; the idle change touches only the STT task's poll timer, which the media-path benches do not run.

## JS main thread, 10 voice sessions (`start:roundtrip-counting-echo-lid-multi`, `scripts/perf/js-thread-probe.mjs`, capture off)

| Version | Run | JS-thread CPU ms/s | Process CPU ms/s |   ELU | Delay p99 ms |
| ------- | --- | -----------------: | ---------------: | ----: | -----------: |
| 0.10.1  | 1   |                 18 |              571 | 0.018 |         12.6 |
| 0.10.1  | 2   |                 18 |              563 | 0.017 |         12.5 |
| 0.10.1  | 3   |                 18 |              566 | 0.017 |         12.5 |
| 0.11.0  | 1   |                 19 |              579 | 0.017 |         12.5 |
| 0.11.0  | 2   |                 18 |              583 | 0.017 |         12.6 |
| 0.11.0  | 3   |                 18 |              567 | 0.017 |         12.7 |

## Voice loopback under load (`start:roundtrip-load`)

| Version | Final p50 / p95 ms | TTS start p50 / p95 ms | Turn success | CPU s per session-min | RSS end MB |
| ------- | ------------------ | ---------------------- | -----------: | --------------------: | ---------: |
| 0.10.1  | 2090 / 2178        | 67 / 485               |        100 % |                  4.36 |        834 |
| 0.11.0  | 2056 / 2171        | 66 / 471               |        100 % |                  4.41 |        863 |

## Capacity mode (`SHERPA_LOAD_CAPACITY=1 SHERPA_LOAD_TURNS=2`)

| Run          | Max legs at SLO | Step that failed | Reason                                                                  | 48-leg p95 ms | 48-leg CPU s per session-min |
| ------------ | --------------: | ---------------- | ----------------------------------------------------------------------- | ------------: | ---------------------------: |
| 0.10.1 run 1 |              40 | 48               | latency (p95 3068 > baseline + 1000)                                    |          3068 |                         2.78 |
| 0.10.1 run 2 |              48 | none             |                                                                         |          3006 |                         2.77 |
| 0.11.0 run 1 |              10 | 12               | 1 of 24 turns: STT heard "Alpha one two three" as "Halfa one two three" |               |                              |
| 0.11.0 run 2 |              48 | none             |                                                                         |          3034 |                         2.86 |
| 0.11.0 run 3 |              40 | 48               | latency (p95 3113)                                                      |          3113 |                         2.86 |

The 0.11.0 run-1 failure was a single recognition error: all 24 finals arrived, latency p95 was 2203 ms (as on 0.10.1), CPU normal. It did not recur in the two further 0.11.0 runs (about 1,100 turns). The loopback runs do not save the STT input, so the audio of that turn cannot be replayed. Capacity is the same on both versions (40–48 legs); 48 legs is this machine's latency limit for both.

## CPU starvation (functional, `start:roundtrip-counting-echo-lid-multi` under `run-starve.py`, throttle 5 ms run / 45 ms stop)

Same harness as the 0.10.0 starvation work (5 sessions, the echo process gets about 10 % of a core once all sessions are connected). Head: 0.11.0 code (#441), 3 runs one at a time; the host also carried another workload (load average 21–46).

| Run | Result                                               | Duration |
| --: | ---------------------------------------------------- | -------: |
|   1 | pass (every phrase echoed, no lost or doubled words) |     37 s |
|   2 | pass                                                 |     45 s |
|   3 | pass                                                 |     47 s |

## Files

| File                                                                                       | Content                                                                                        |
| ------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------- |
| `media-path-0.10.1-a194e23.json`, `media-path-0.11.0-pr439.json`, `media-path-compare.txt` | `scripts/perf/run-perf-baseline.sh` reports and `compare-perf.mjs` output                      |
| `idle-agents-alternating.txt`                                                              | Five alternating `idle_agents` runs (run, version, commit, µs per agent-second, load averages) |
| `js-thread-<version>-run<n>.txt`                                                           | `js-thread-probe.mjs` lines of the 10-session runs                                             |
| `roundtrip-load-<version>.jsonl`                                                           | `perf_probe` lines of `start:roundtrip-load`                                                   |
| `roundtrip-capacity-<version>-run<n>.jsonl`                                                | `perf_probe` lines of the capacity runs                                                        |
| `starve-5-45-summary.txt`                                                                  | Starvation run summary                                                                         |
