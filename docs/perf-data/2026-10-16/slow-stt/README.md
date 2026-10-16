# Slow-STT: longest `VoiceAgent::process_inbound_pcm` call, nwr 0.9.48 / 0.10.0 / 0.11.2

Measured 2026-10-16 on the shared Mac (load average about 3-4, no `/tmp/nwr-load.lock`, no `yes`), `--release`,
`--test-threads=1 --nocapture`. Durations are `Instant::elapsed().as_millis()` (whole ms, truncated) measured
around each `process_inbound_pcm` call by the test harness; frames are paced from an anchor every 20 ms.

## Table (longest single inbound call)

| Scenario                                                                   | 0.9.48              | 0.10.0              | 0.11.2              | Test                                                                                                      |
| -------------------------------------------------------------------------- | ------------------- | ------------------- | ------------------- | --------------------------------------------------------------------------------------------------------- |
| 3.3 s late final                                                           | 3302 ms (test FAIL) | 3303 ms (test FAIL) | 0 ms (< 1 ms), PASS | `slow_final_agent_deaf_fullstack_test` > `agent_keeps_hearing_after_a_slow_final`                         |
| 3.3 s late final + 3 s late `Finalized`                                    | 6305 ms (FAIL)      | 6306 ms (FAIL)      | 0 ms (< 1 ms), PASS | same file > `agent_keeps_hearing_after_a_slow_final_and_slow_finalized`                                   |
| 10 s late final, next utterance after it (VAD test)                        | 10002 ms (FAIL)     | 10003 ms (FAIL)     | 0 ms (< 1 ms), PASS | `slow_final_next_utterance_fullstack_test` > `vad_events_of_next_utterance_on_time_during_10s_slow_final` |
| same, with agent TTS playing + barge-in                                    | 10002 ms (FAIL)     | 10002 ms (FAIL)     | 0 ms (< 1 ms), PASS | same file > `barge_in_on_time_during_10s_slow_final`                                                      |
| Stream opens refused (300 x, about 60 s) while three utterances are spoken | 57023 ms (FAIL)     | 57001 ms (FAIL)     | 0 ms (< 1 ms), PASS | `refused_open_streak_fullstack_test` > `sixty_second_refused_open_streak_keeps_events_flowing`            |

Extra rows from the same binaries:

| Scenario                                                                                        | 0.9.48                                                                                 | 0.10.0                   | 0.11.2                  |
| ----------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- | ------------------------ | ----------------------- |
| 3.3 s late final on a 1.5 s short turn (`..._on_a_short_turn`)                                  | 3303 ms (FAIL)                                                                         | 3301 ms (FAIL)           | 2 ms, PASS              |
| Next stream open never answers headers (`agent_keeps_hearing_while_the_next_stream_open_hangs`) | > 20 s (hung: the test's own 20 s bound hit in turn 4; `agent.stop()` also > 5 s) FAIL | > 20 s (hung, same) FAIL | 1 ms, `stop_ms=0`, PASS |
| `stop_returns_while_a_stream_open_hangs`                                                        | FAIL (stop() > 3 s)                                                                    | FAIL (stop() > 3 s)      | PASS                    |
| `open_that_never_answers_is_retried_on_a_fresh_connection`                                      | FAIL                                                                                   | FAIL                     | PASS                    |

Notes:

- No scenario was hung beyond the 600 s binary timeout: every old-version binary finished (deaf 457-458 s, next_utterance 49-50 s,
  refused 85 s wall). All old-version failures are the expected "0.11 behaviour required" assertions; the longest call is printed
  before the assertions run.
- 0.9.48 and 0.10.0 are indistinguishable in this area (within 2 ms).
- 0.11.2 had a 1 ms resolution floor: every long-call value is 0 ms except two turns of the short-turn test (1 and 2 ms) and one
  turn of the hung-open test (1 ms).
- The 10 s scenario measured 10.0 s on the old releases (the published 0.10.1 number is about 8 s; this test sets
  `SPEECH_STT_FINALIZE_WAIT_MS=15000`, which may explain the difference; not investigated).
- The refused-streak blocked call measured 57 s on the old releases (the mock refuses 300 opens, `rejected_opens=300`); not the full 60 s.
- In old versions `h.agent.wait_stt_idle(20 s)` was replaced by an unconditional 20 s sleep, so "idle" is always true there.
  The 0.11.2 binaries are unmodified.

## 0.11.2 (unmodified)

No source or test edits. The existing tests already print the numbers (`describe()` per-turn `max_call_ms`, `slow calls`,
`process_inbound_pcm calls=N p99_ms=`; the Harness prints `max_call_ms=` / `p99_call_ms=`), so no `eprintln!` was added.
All 9 tests (1 + 6 + 2) passed.

Command (from `$FEATURE/nwr-0.11.2`):

    cargo test --release -p node-webrtc-rust-vendor-cluster-speech --test slow_final_agent_deaf_fullstack_test --test slow_final_next_utterance_fullstack_test --test refused_open_streak_fullstack_test -- --test-threads=1 --nocapture

Log: `0.11.2-run-20261016-152021.log` (exit 0; refused 65 s, deaf 331 s, next_utterance 29 s).

## 0.9.48 and 0.10.0 (test files copied from `release/0.11.2`)

Copied unchanged from `git -C node-webrtc-rust show release/0.11.2:<path>` into `crates/vendor-cluster-speech/tests/`:
`mock_server.rs` (overwrites the older file; additive: `final_delay_ms`, `drop_stream_on_finalize`, `open_header_delay_*`),
`fullstack_common/mod.rs`, `slow_final_agent_deaf_fullstack_test.rs`, `slow_final_next_utterance_fullstack_test.rs`,
`refused_open_streak_fullstack_test.rs`. Only these helper modules are `mod`-included. Nothing under `src/` was touched in any
worktree. `git -C <wt> diff --stat` (identical for both):

    crates/vendor-cluster-speech/tests/mock_server.rs | 30 +++++++++++++++++++++++   (the 0.11.2 file, unmodified)
    + 4 untracked new files (fullstack_common/mod.rs and the three *_fullstack_test.rs)

Edits on top of the copies (full diff against the 0.11.2 files: `0.10.0-test-edits.diff`, `0.9.48-test-edits.diff`; identical):

1. `TtsConfig.sentence_case` does not exist before 0.11: removed the `sentence_case: Default::default(),` line in
   `fullstack_common/mod.rs` and `slow_final_agent_deaf_fullstack_test.rs` (only compile errors).
2. `VoiceAgent::wait_stt_idle` missing: in `slow_final_next_utterance_fullstack_test.rs` (2 places) and
   `refused_open_streak_fullstack_test.rs` (1 place) replaced by `tokio::time::sleep(Duration::from_secs(20)).await; true`.
3. `VoiceAgent::stt_audio_bytes_sent` missing: in `refused_open_streak_fullstack_test.rs` `bytes_sent` is `0_u64` and the final
   audio-lost `assert_eq!(bytes_received, bytes_sent)` was dropped. That assertion is not part of the measurement.
4. `fullstack_common/mod.rs`: `HUNG_CALL_BOUND` raised from 20 s to 120 s so the harness waits out the old 57 s block instead of panicking
   at 20 s (the measured value would otherwise be lost). This does not change the timing loop, cadence, mock delays or how the call is
   measured. Plus one `eprintln!("[measure] process_inbound_pcm HUNG ...")` before the existing hang `panic!` (never fired).
5. `slow_final_agent_deaf_fullstack_test.rs` keeps its own 20 s `HUNG_CALL_BOUND` (it records the hang and continues), which is why the hung-open
   scenario reads "> 20 s (hung)".

Nothing else needed changing: the max call is printed (`describe()` / `Harness::timeline` / `finish()`) before any assertion in all tests.

Commands (per worktree; build then run each binary directly, `timeout 600` each, script at
`/private/tmp/claude-501/-Users-akirilyuk-GIT-webrtc-cloud/bf65213a-e07d-5505-b0e3-80aa6b0c450f/scratchpad/run-old.sh`):

    cargo test --release -p node-webrtc-rust-vendor-cluster-speech --test slow_final_agent_deaf_fullstack_test --test slow_final_next_utterance_fullstack_test --test refused_open_streak_fullstack_test --no-run
    timeout 600 target/release/deps/<test-binary> --test-threads=1 --nocapture

## Logs (this directory)

- `0.11.2-run-20261016-152021.log`
- `0.10.0-build.log` (first build failed on `sentence_case`), `0.10.0-build2.log` (ok); `0.9.48-build.log`, `0.9.48-build2.log` likewise
- `0.10.0-{slow_final_next_utterance,refused_open_streak,slow_final_agent_deaf}_fullstack_test.log` + `.exit`
- `0.9.48-{slow_final_next_utterance,refused_open_streak,slow_final_agent_deaf}_fullstack_test.log` + `.exit`
- `0.10.0-test-edits.diff`, `0.9.48-test-edits.diff`
