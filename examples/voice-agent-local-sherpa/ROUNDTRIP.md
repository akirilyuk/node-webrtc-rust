# Sherpa TTS → STT roundtrip

Node integration test for on-device Sherpa speech: **text → TTS → WebRTC → STT → text**, with VAD and `gateStt` enabled on the listener.

Implementation: [`src/roundtrip.ts`](./src/roundtrip.ts).

## Can gaps come from our VAD?

**Yes — for timing inside and after an utterance.** **No — `minSilenceDurationMs` is not a “pause between batch phrases” knob.**

| Source                                     | What it does                                                                                                                                                                                                                                             |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **`minSilenceDurationMs` (1300 ms preset)** | How long silence must last **during** one utterance before VAD sees `SpeechEnd` (“maybe done”). Short pauses under ~1.3 s should not end the turn.                                                                                                           |
| **`sttGateHoldMs` (1000 ms default)**      | With **`gateStt: true`**, keep feeding STT after that internal speech end; **`user_speaking_end` is emitted when this hold expires** (not on the first short gap). If the user speaks again during hold, the utterance continues and no end event fires. |
| **Endpoint tail**                          | `minSilence` clamped **400–600 ms** of synthetic silence pushed to STT after hold reaches zero, then `finalize_utterance` (Rust; not duplicated on the harness speaker track).                                                                           |
| **`speechPadMs` / `minSpeechDurationMs`**  | Pre-roll and minimum voiced time before `user_speaking_start` — not inter-phrase gaps.                                                                                                                                                                   |

**Between batch phrases**, separation is:

1. Wait for `user_speech_final` (or the collector’s post-`user_speaking_end` fallback, aligned with hold + tail).
2. Trailing silence on the speaker track (duration derived from `sttGateHoldMs` + tail) so the listener still receives PCM while the gate drains.
3. Optional **`SHERPA_ROUNDTRIP_GAP_S`** — extra wall-clock silence (default `0`).

**Real-time pacing:** `streamSilence()` sends one 20 ms frame every ~20 ms wall time (same pattern as `browser-cosine-chat`), so `sttGateHoldMs` and trailing silence behave closer to a live call than a PCM burst.

## Architecture (two VoiceAgents)

The roundtrip uses **two separate `VoiceAgent` instances** on two WebRTC peers — not one agent with a PCM relay loopback.

```text
┌─────────────────────────────┐         WebRTC          ┌─────────────────────────────┐
│  Speaker (agent peer)       │   agentOut ──────────►  │  Listener (user peer)       │
│  VoiceAgent                 │      userInbound        │  VoiceAgent                 │
│  • Sherpa TTS only          │                         │  • Sherpa STT               │
│  • VAD disabled             │                         │  • VAD + gateStt (defaults) │
└─────────────────────────────┘                         └─────────────────────────────┘
```

This matches production:

- **Speaker** plays synthesized audio on its outbound track.
- **Listener** receives remote audio on its inbound track (like a phone call).
- The listener does **not** hear its own TTS; the browser does not loop playback into the mic send path either (separate capture vs speaker output, plus AEC).

The **listener** does not play TTS — its `bargeIn` setting does not stop the speaker. See [VOICE-VAD-AND-BARGE-IN.md](../../packages/sdk/VOICE-VAD-AND-BARGE-IN.md) for two-peer vs single-agent layouts.

## Quick start

```bash
cd node-webrtc-rust
npm run build:native   # after Rust changes
npm run download-stt:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa

export SHERPA_STT_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06"
export SHERPA_TTS_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low"

npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

## Run modes

| Mode                  | Command                                                                                                                              |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| **Batch (default)**   | `npm run start:roundtrip` — 5 built-in sentences + similarity table                                                                  |
| **Counting 1–20**     | `npm run start:roundtrip-counting` — one long utterance, single final (see below)                                                    |
| **Counting echo**     | `npm run start:roundtrip-counting-echo` — Agent1↔Agent2, one…ten both legs (see below)                                               |
| **Barge recovery**    | `npm run start:roundtrip-counting-barge-recovery` — full echo → barge → partial → recovery (see below)                               |
| **Utterance timing**  | `npm run start:roundtrip-utterance-timing` — `user_speaking_end` → `user_speech_final` within 500 ms (see below)                     |
| **Two phrases**       | `npm run start:roundtrip-two-phrases` — count, pause, second sentence → **2×** `user_speech_final` (see below)                       |
| **Single phrase**     | `npm run start:roundtrip -- "I love America"`                                                                                        |
| **Single via env**    | `SHERPA_ROUNDTRIP_PHRASE="Hello world" npm run start:roundtrip`                                                                      |
| **Semantic barge-in** | `npm run start:roundtrip-barge-in` — tone must not barge; spoken phrase must (see [§ Semantic barge-in E2E](#semantic-barge-in-e2e)) |

All modes above are exercised in CI — see [§ CI (GitHub Actions)](#ci-github-actions).

## STT lifecycle evaluators

Shared Vitest helpers in [`src/roundtrip-stt-lifecycle-helpers.ts`](./src/roundtrip-stt-lifecycle-helpers.ts) assert **VAD/STT utterance lifecycle** event order (no Sherpa models required). Wired into counting, two-phrases, utterance-timing, barge-in, and barge-recovery E2E scripts.

| Evaluator                                | Asserts                                                                                                      |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `evaluateVadSttSessionOpen`              | Each VAD `SpeechStart`: `vad_triggered` → `user_stt_start` → `stt_stream_start` (tight gap, default ≤100 ms) |
| `evaluateUtteranceSessionCloseWithFinal` | Successful close: `stt_stream_end` → `user_stt_end` → `user_speaking_end` → `user_speech_final`              |
| `evaluateC1NotFoundPath`                 | Tone / no-transcript: `stt_stream_end` → `user_stt_not_found` → `user_stt_end`, **no** `user_speech_final`   |
| `evaluateNoPartialWithoutFinal`          | No orphan `user_speech_partial` without a final or C1                                                        |
| `evaluateSttLifecycleOnBargePath`        | After `agent_speaking_start`: `vad_triggered` before qualifying partial; STT stream open before partial      |
| `evaluateNormalUtteranceLifecycle`       | Open + close + no orphans (counting, two-phrases, utterance-timing)                                          |
| `evaluateBargePathLifecycle`             | Barge path open + close + no orphans (barge-in phase 3)                                                      |
| `evaluateTonePhaseLifecycle`             | Phase 2 tone: no `barge_in`, no `user_speech_final`; optional C1                                             |

```bash
npm run test:roundtrip-counting --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
# includes src/roundtrip-stt-lifecycle.test.ts
```

When debugging failures, grep **`[speech]`** lines for `vad_triggered`, `stt_stream_*`, and `user_stt_*` before re-running — see [Debug logging](#debug-logging-e2e-failures).

## Counting roundtrip (one utterance, one final)

[`src/roundtrip-counting.ts`](./src/roundtrip-counting.ts) plays **one** long TTS phrase — the words _one_ through _twenty_ — and asserts the listener does **not** split it into multiple STT finals or extra `user_speaking_end` events (regression for mid-utterance VAD gaps while counting).

| Check               | Requirement                                                      |
| ------------------- | ---------------------------------------------------------------- |
| `user_speech_final` | **Exactly 1**                                                    |
| `user_speaking_end` | **Exactly 1**                                                    |
| Transcript          | At least **16/20** number words in the final text (configurable) |

```bash
npm run build:native
# models + SHERPA_*_MODEL_PATH as in Quick start
npm run start:roundtrip-counting --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Unit tests (no Sherpa models): `npm run test:roundtrip-counting --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa`

## Counting echo roundtrip (Agent1 ↔ Agent2, multi-round)

[`src/roundtrip-counting-echo.ts`](./src/roundtrip-counting-echo.ts) uses **two** full VoiceAgents on one loopback. **Agent 2** always replies with `You said: {recognized utterance}` (same as the multi-client `voice-handler`).

Each **round** is two legs (both must be **one** `user_speech_final` and **one** `user_speaking_end`):

| Round                 | Agent 1 speaks                             | Checks                                                                   |
| --------------------- | ------------------------------------------ | ------------------------------------------------------------------------ |
| **1 — counting**      | _one_ … _ten_                              | ≥8/10 number words; echo leg includes “you said” + ≥60% number retention |
| **2 — long sentence** | _This is a very long sentence…_ (built-in) | ≥75% word similarity; echo leg ≥60% similarity + “you said”              |

```bash
npm run build:native
# models + SHERPA_*_MODEL_PATH as in Quick start
npm run start:roundtrip-counting-echo --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

| Env                                 | Default                | Purpose                                 |
| ----------------------------------- | ---------------------- | --------------------------------------- |
| `SHERPA_COUNTING_PHRASE`            | `one two … ten`        | Round 1 source phrase                   |
| `SHERPA_ECHO_LONG_SENTENCE`         | built-in long sentence | Round 2 source phrase                   |
| `SHERPA_COUNTING_ECHO_MIN_WORDS`    | `8`                    | Min number tokens on echo leg (round 1) |
| `SHERPA_ECHO_MIN_SIMILARITY`        | `0.75`                 | Min word match round 2 leg A            |
| `SHERPA_ECHO_LEG_MIN_SIMILARITY`    | `0.6`                  | Min word match round 2 echo leg B       |
| `SHERPA_ECHO_MIN_RETENTION`         | `0.6`                  | Echo content retention (both rounds)    |
| `SHERPA_COUNTING_INTER_LEG_GAP_S`   | `0.5`                  | Silence between A and B within a round  |
| `SHERPA_COUNTING_INTER_ROUND_GAP_S` | `1.0`                  | Silence between round 1 and round 2     |

Other `SHERPA_COUNTING_*` vars apply (`TIMEOUT_MS`, `VERBOSE`, etc.).

## Counting barge-in recovery roundtrip

[`src/roundtrip-counting-barge-recovery.ts`](./src/roundtrip-counting-barge-recovery.ts) extends the echo harness with a **barge-in** step and a **recovery** round:

| Step             | What happens                                                                                     | Pass criteria                                                                                                                                          |
| ---------------- | ------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **1 — baseline** | Agent1 counts 1–10 → Agent2 → `You said: …` → Agent1                                             | Full echo (same as counting echo round 1)                                                                                                              |
| **2 — barge**    | Agent1 counts again → Agent2 starts `You said: …` → Agent1 Sherpa TTS barge phrase on `agentOut` | Agent1 hears **partial** echo (≤6 number words, similarity ≤55% vs full phrase); Agent2 **`user_speech_final`** matches barge phrase (≥60% similarity) |
| **3 — recovery** | Agent1 speaks recovery phrase → full `You said: …` again                                         | Echo leg includes “you said” and passes sentence echo checks                                                                                           |

```bash
npm run build:native
# models + SHERPA_*_MODEL_PATH as in Quick start
npm run start:roundtrip-counting-barge-recovery --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

| Env                                      | Default                                | Purpose                                                           |
| ---------------------------------------- | -------------------------------------- | ----------------------------------------------------------------- |
| `SHERPA_BARGE_RECOVERY_PHRASE`           | `hello testing recovery one two three` | Round 3 source phrase                                             |
| `SHERPA_BARGE_RECOVERY_DELAY_MS`         | `400`                                  | Ms after Agent2 TTS starts before barge TTS on `agentOut`         |
| `SHERPA_BARGE_RECOVERY_BARGE_PHRASE`     | `stop now please`                      | Sherpa TTS phrase Agent1 plays to barge Agent2                    |
| `SHERPA_BARGE_RECOVERY_TONE_S`           | `1.0`                                  | Silence tail after barge TTS on `agentOut`                        |
| `SHERPA_BARGE_RECOVERY_MAX_NUMBER_WORDS` | `6`                                    | Max number tokens on interrupted leg B                            |
| `SHERPA_BARGE_RECOVERY_MAX_SIMILARITY`   | `0.55`                                 | Max word similarity vs full `You said: …` text on interrupted leg |

Unit tests include `roundtrip-counting-barge-recovery.test.ts` in `npm run test:roundtrip-counting`.

## Two-phrase roundtrip (multi-client turn-taking)

[`src/roundtrip-two-phrases.ts`](./src/roundtrip-two-phrases.ts) simulates **two separate user turns** on one listener (same as one browser tab in multi-client):

| Turn      | TTS phrase                                       | Expected                                                   |
| --------- | ------------------------------------------------ | ---------------------------------------------------------- |
| **1**     | `one two … ten`                                  | `user_speaking_end` then `user_speech_final` (gap ≤500 ms) |
| **pause** | wall-clock silence (`interPhraseSilenceSeconds`) | Turn 1 fully finalized before turn 2                       |
| **2**     | `I am done speaking`                             | Second `user_speaking_end` + `user_speech_final`           |

In **multi-client**, each `user_speech_final` should trigger `voice-handler` → `You said: …` (**two agent replies**). This roundtrip does not run the handler; it only proves the Rust pipeline emits **2 finals**.

```bash
npm run build:native
npm run start:roundtrip-two-phrases --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

| Env                             | Default              | Purpose                                                           |
| ------------------------------- | -------------------- | ----------------------------------------------------------------- |
| `SHERPA_TWO_PHRASE_FIRST`       | `one two … ten`      | Turn 1                                                            |
| `SHERPA_TWO_PHRASE_SECOND`      | `I am done speaking` | Turn 2                                                            |
| `SHERPA_TWO_PHRASE_EXTRA_GAP_S` | `1.5`                | Extra silence between turns (on top of VAD-derived post-TTS tail) |

## Utterance timing roundtrip (`user_speaking_end` ↔ `user_speech_final`)

[`src/roundtrip-utterance-timing.ts`](./src/roundtrip-utterance-timing.ts) plays one counting phrase and asserts **`user_speaking_end` arrives at most 500 ms before `user_speech_final`** (regression for Sherpa finalize lag with the STT gate closed early).

```bash
npm run build:native
npm run start:roundtrip-utterance-timing --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

| Env                                   | Default         | Purpose                                |
| ------------------------------------- | --------------- | -------------------------------------- |
| `SHERPA_MAX_SPEAKING_END_TO_FINAL_MS` | `500`           | Max allowed gap between the two events |
| `SHERPA_UTTERANCE_TIMING_PHRASE`      | `one two … ten` | Spoken phrase                          |

| Env                                | Default            | Purpose                    |
| ---------------------------------- | ------------------ | -------------------------- |
| `SHERPA_COUNTING_PHRASE`           | `one two … twenty` | Override spoken text       |
| `SHERPA_COUNTING_TIMEOUT_MS`       | `90000`            | Wait for transcript        |
| `SHERPA_COUNTING_MIN_NUMBER_WORDS` | `16`               | Min number tokens in final |
| `SHERPA_COUNTING_VERBOSE`          | off                | Log each speech event      |

### Default batch sentences

1. `I love America`
2. `The weather is nice today.`
3. `Hello world`
4. `Open the browser please`
5. `Speech recognition works locally`

## Similarity check

After each phrase, input and STT output are compared:

1. **Normalize** both strings: `toLowerCase()`, remove punctuation, collapse whitespace.
2. **Word similarity** = (input words found as whole tokens in recognized text) / (input word count).
3. **Pass** if similarity ≥ `SHERPA_ROUNDTRIP_MIN_SIMILARITY` (default `0.75`) and recognized text is non-empty.

Examples:

| Input (normalized)          | Recognized (normalized)              | Similarity                   |
| --------------------------- | ------------------------------------ | ---------------------------- |
| `i love america`            | `i love america`                     | 100%                         |
| `i love america`            | `thy love america`                   | 67% (fails at 75% threshold) |
| `the weather is nice today` | `the weather weather is nice to day` | 80%                          |

Exact character match is not required; STT may split or repeat words (`to day` vs `today`).

## Timing: VAD vs explicit silence

Gaps and pauses come from **two layers**: the **VoiceAgent VAD/STT pipeline** (listener) and **optional explicit PCM** on the speaker outbound track (test harness only).

### Within one phrase (listener VAD + gateStt)

| Setting                | Default                         | Role                                                                                                 |
| ---------------------- | ------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `minSpeechDurationMs`  | 250                             | Voice must be present this long before `user_speaking_start`                                         |
| `minSilenceDurationMs` | 1300 (`VOICE_AGENT_VAD_PRESET`) | Silence this long before “maybe done” / gate hold (then `sttGateHoldMs` grace)                      |
| `speechPadMs`          | 300                             | Pre-roll ring size only (`speechPadMs + minSpeechDurationMs` ≈ 550 ms buffered before `SpeechStart`) |
| `gateStt`              | true                            | STT only while gate is open                                                                          |
| `gateSttOpenOnPending` | true                            | Gate opens during VAD **pending** speech (before `SpeechStart`) — covers WebRTC lead-in              |
| `sttGateHoldMs`        | 1000                            | After `SpeechEnd`, keep feeding STT for this many ms (trailing phonemes + relay)                     |
| Endpoint tail          | `minSilence` clamped 400–600 ms | Extra silence pushed to STT after hold expires, then `finalize_utterance` (Rust only)                |

**`minSilenceDurationMs` is not a gap between batch phrases.** It only controls how long silence must last **inside** an utterance before VAD declares speech ended. Natural pauses under the preset (default **1300 ms** + **1000 ms** gate hold) should not split one phrase.

### After each phrase (listener finalize path)

When the listener’s VAD fires `user_speaking_end`:

1. `sttGateHoldMs` counts down on incoming PCM frames (needs real audio/silence on the wire).
2. Hold expiry → endpoint tail silence → Sherpa `finalize_utterance` → `user_speech_final`.

The roundtrip waits for **`user_speech_final`** (or timeout / partial fallback) before starting the next phrase. That wait is the main **logical** separator between batch sentences.

### Trailing silence after TTS (speaker outbound, VAD-aligned)

After each `sendTextToTTS`, the harness waits for **`agent_speaking_end`** (or a phrase-length estimate cap), then streams trailing silence on the speaker outbound track **at real time** while the listener wait for `user_speech_final` is already in flight:

```text
postTtsSilenceS = (sttGateHoldMs + minSilenceDurationMs + margin) / 1000
```

Rust injects the STT **endpoint tail** on finalize (`minSilence` clamped 400–600 ms) — the harness does **not** add that again on the wire.

Defaults ≈ **1.55 s** trailing silence (800 + 500 + 250 ms of 20 ms frames) plus gate hold. End-to-end finalize after the user stops talking targets **~1.5–2 s** (`minSilence` + `sttGateHold` + endpoint tail). The old **~2.3 s** post-TTS padding and **multi-second estimate playback waits** after TTS had already ended were the main roundtrip slowdown.

### Harness playback timing (`AgentSpeakingEndLatch`)

Production apps do not need this — it is **test-harness-only** code in [`src/roundtrip-counting.ts`](./src/roundtrip-counting.ts), shared by all `start:roundtrip*` scripts.

#### Why it exists

Each phrase calls `playSpeakerTtsWithPostSilence`, which must:

1. Send text to Sherpa TTS on the **speaker** `VoiceAgent`.
2. Wait until outbound playback has **actually finished** before streaming trailing silence.
3. Stream trailing silence at real time so the **listener** VAD/STT gate can drain and emit `user_speech_final`.

Step 2 used to be a **fixed wall-clock estimate** (`~900 ms × word count + 3 s`, capped at 45 s). Piper often finishes much sooner; `agent_speaking_end` can fire at ~3 s while the harness still slept until ~5.7 s. That added **seconds of dead air** per phrase and made CI roundtrips feel stuck (the `[listener] still waiting for transcript (10s)` line is only a progress log every 10 s, not an extra wait).

#### Why not wait on the listener?

`agent_speaking_start` / `agent_speaking_end` are emitted by the **VoiceAgent that plays TTS** (the speaker). They appear on that agent’s `speechEvents()` async iterator.

The listener’s `speechEvents()` stream carries **user-leg** events (`user_speaking_*`, `user_speech_*`, `barge_in`). It does **not** include `agent_speaking_end` for the remote peer’s TTS. Waiting on the listener for agent end never resolves; the harness fell back to the long estimate every time.

#### Components

| Export                                                                     | Role                                                                                                                                                                                                                      |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **`AgentSpeakingEndLatch`**                                                | Counts `agent_speaking_end` on the speaker stream. `waitForNext(timeoutMs)` resolves on the **next** end event after the call (baseline pattern — safe across multi-phrase runs).                                         |
| **`startSpeakerSpeechPump(speaker, latch?)`**                              | For **speaker-only** agents (no `ListenerUtteranceCollector` on the same `VoiceAgent`). If the agent also has a collector, pass `agentEndLatch` into the collector instead — **one `speechEvents()` consumer per agent**. |
| **`waitAgentPlaybackEndRace({ phrase, waitForAgentSpeakingEnd, capMs })`** | `Promise.race` between latch wait and `estimateTtsPlaybackMs(phrase)`. Logs `playback ended (agent_speaking_end)` or `playback ended (estimate cap …)`.                                                                   |
| **`playSpeakerTtsWithPostSilence({ …, agentSpeakingEndLatch })`**          | `sendTextToTTS` → playback race → `streamSilence(postTtsSilenceS)`.                                                                                                                                                       |
| **`postTtsSilenceSeconds(config)`**                                        | `(sttGateHoldMs + minSilenceDurationMs + margin) / 1000` — does **not** include Rust endpoint tail.                                                                                                                       |

Echo / two-agent scripts use **one latch per TTS-speaking agent** (`agent1EndLatch`, `agent2EndLatch`) and pass the matching latch into `playTtsAndCollect` / `playSpeakerTtsWithPostSilence`.

#### Per-phrase timeline (typical)

```text
  sendTextToTTS
       │
       ├─► [parallel] listener already waiting for user_speech_final
       │
       ├─► waitAgentPlaybackEndRace
       │        ├─ agent_speaking_end  (~3 s)  ◄── preferred
       │        └─ or estimate cap      (~5.7 s for short phrase)  ◄── safety only
       │
       ├─► streamSilence(postTtsSilenceS)   (~1.75 s real-time PCM)
       │
       └─► listener: hold + endpoint tail → user_speaking_end → user_speech_final (~1–1.5 s after TTS end)
```

Example log (single phrase `"I love America"`):

```text
[speaker] playback wait ≤5.7s (agent_speaking_end or estimate)
[speech] [speaker] +3516ms agent_speaking_end
[speaker] playback ended (agent_speaking_end)
[speaker] post-TTS silence 1.8s
[speech] [listener] +4794ms user_speaking_end
[speech] [listener] +4794ms user_speech_final "I love America"
```

~1.3 s from `agent_speaking_end` to final — within the ~1.5–2 s target.

#### Wiring checklist (new roundtrip script)

1. Create `const agentEndLatch = new AgentSpeakingEndLatch()` (or two latches for echo).
2. Call `startSpeakerSpeechPump(speaker, agentEndLatch)` **once** after `speaker.start()` — before the first TTS.
3. Start the listener collector pump (`ListenerUtteranceCollector.startPump()`).
4. For each phrase: `const p = collector.waitForNext(…)` **before** TTS; then `playSpeakerTtsWithPostSilence({ agentSpeakingEndLatch: agentEndLatch, … })`; then `await p`.
5. Do **not** add a second `speaker.speechEvents()` loop.

#### Scripts using the harness

| Script                                 | Latches                                                                   |
| -------------------------------------- | ------------------------------------------------------------------------- |
| `roundtrip.ts`                         | 1× speaker                                                                |
| `roundtrip-counting.ts`                | 1× speaker                                                                |
| `roundtrip-utterance-timing.ts`        | 1× speaker (via `playTtsAndCollect`)                                      |
| `roundtrip-two-phrases.ts`             | 1× speaker                                                                |
| `roundtrip-counting-echo.ts`           | `agent1EndLatch`, `agent2EndLatch`                                        |
| `roundtrip-counting-barge-recovery.ts` | `agent1EndLatch`, `agent2EndLatch` (+ latch on agent2 for barge echo leg) |

`roundtrip-barge-in.ts` uses its own phase collector (no `playSpeakerTtsWithPostSilence` for the main flow).

### Between phrases (batch)

| Mechanism                    | Default                  | Purpose                                                    |
| ---------------------------- | ------------------------ | ---------------------------------------------------------- |
| Wait for `user_speech_final` | always                   | Next TTS starts only after previous utterance finalized    |
| Trailing silence (above)     | ~1.75 s from VAD timings | Lets hold + finalize complete on the listener              |
| `SHERPA_ROUNDTRIP_GAP_S`     | **0**                    | Extra explicit silence between phrases; **off by default** |

With `SHERPA_ROUNDTRIP_GAP_S=0` (default), **inter-phrase gaps come from VAD-driven finalize timing plus VAD-aligned trailing silence**, not from a separate fixed 1 s gap. The harness **must** stream that trailing PCM at real time (in parallel with waiting for `user_speech_final`) so `sttGateHoldMs` can count down on the wire; without it, the next phrase can start before finalize and STT can bleed across phrases.

Set `SHERPA_ROUNDTRIP_GAP_S=1` (or higher) only if you need extra separation beyond what hold + trailing silence provide.

### Before the first phrase

| Mechanism                   | Default | Purpose                                                                         |
| --------------------------- | ------- | ------------------------------------------------------------------------------- |
| `SHERPA_ROUNDTRIP_WARMUP_S` | 0.6 s   | Explicit silence on speaker outbound to prime WebRTC before first TTS (not VAD) |

## Environment variables

| Variable                          | Default | Purpose                                                                  |
| --------------------------------- | ------- | ------------------------------------------------------------------------ |
| `SHERPA_ROUNDTRIP_PHRASE`         | —       | Single phrase (skips 5-sentence batch)                                   |
| `SHERPA_ROUNDTRIP_MIN_SIMILARITY` | `0.75`  | Min word-match ratio to pass                                             |
| `SHERPA_ROUNDTRIP_TIMEOUT_MS`     | `45000` | Per-phrase STT timeout                                                   |
| `SHERPA_ROUNDTRIP_WARMUP_S`       | `0.6`   | Speaker warmup silence (seconds) before first TTS                        |
| `SHERPA_ROUNDTRIP_GAP_S`          | `0`     | **Extra** silence between phrases (seconds); VAD trailing usually enough |
| `SHERPA_ROUNDTRIP_VERBOSE`        | unset   | Set to `1` for per-frame VAD/STT logs                                    |

Model paths: `SHERPA_STT_MODEL_PATH`, `SHERPA_TTS_MODEL_PATH` (see main [README](./README.md)).

## Listener defaults (Sherpa example)

From [`src/resolve-voice-config.ts`](./src/resolve-voice-config.ts):

```typescript
vad: {
  enabled: true,
  threshold: 0.05,
  minSpeechDurationMs: 250,
  speechPadMs: 300,
  gateStt: true,
  sttGateHoldMs: 1000,
  bargeIn: { enabled: true, flushTts: true },
}
```

`minSilenceDurationMs` is omitted → library default **1300 ms** (see `VadConfig` in `crates/speech`).

## Expected output

```text
=== Summary ===
| # | Similarity | OK | Input | Recognized |
|---|------------|-----|-------|------------|
| 1 | 100% | yes | I love America | I LOVE AMERICA |
...

Roundtrip OK — 5 phrase(s) passed similarity check.
```

Exit code `1` if any phrase is empty or below the similarity threshold.

## Semantic barge-in E2E

[`src/roundtrip-barge-in.ts`](./src/roundtrip-barge-in.ts) — loopback with production `VOICE_AGENT_VAD_PRESET` + Sherpa STT. Tests **`requireSttPartial`** (default): interrupt only when STT sees words, not on noise.

### What barge-in is

**Barge-in** = the **listener agent** (playing TTS) stops playback and emits `barge_in` when the user is understood to have spoken over the agent.

With `requireSttPartial: true` (default in `VOICE_AGENT_VAD_PRESET`):

1. VAD `SpeechStart` while agent TTS plays → **`vad_triggered`** → **`user_stt_start`** → **`stt_stream_start`** (STT opens on VAD, not continuous pre-VAD feed).
2. First qualifying `user_speech_partial` → flush TTS + `barge_in` → `agent_speaking_end`.
3. Coughs / pure tones → often **C1** (`user_stt_not_found`) → **no barge** (UX improvement).

Set `requireSttPartial: false` to restore immediate energy-VAD barge on the same `SpeechStart` (legacy, noisier).

| Phase | Interrupt                                              | Pass criteria                                                                                                          |
| ----- | ------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| 1     | None                                                   | Full phrase received on `userInbound`                                                                                  |
| 2     | 440 Hz tone on `userOut`                               | **No** `barge_in`; optional **C1** path; received audio ≥ ~75% of phase 1                                              |
| 3     | Sherpa TTS `SHERPA_BARGE_IN_BARGE_PHRASE` on `userOut` | **`vad_triggered` → STT open → `user_speech_partial` → `barge_in` → `agent_speaking_end`**; lifecycle close with final |

```bash
npm run build:native
npm run test:roundtrip-counting --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run test:roundtrip-barge-in --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run start:roundtrip-barge-in --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Event-order logic: [`src/roundtrip-barge-in-helpers.ts`](./src/roundtrip-barge-in-helpers.ts) + [`src/roundtrip-stt-lifecycle-helpers.ts`](./src/roundtrip-stt-lifecycle-helpers.ts) (Vitest, no Sherpa models).

| Variable                               | Default           | Purpose                                    |
| -------------------------------------- | ----------------- | ------------------------------------------ |
| `SHERPA_BARGE_IN_PHRASE`               | long sentence     | Agent TTS under test                       |
| `SHERPA_BARGE_IN_BARGE_PHRASE`         | `stop now please` | User-leg TTS for phase 3                   |
| `SHERPA_BARGE_IN_DELAY_MS`             | `700`             | Ms after agent TTS starts before interrupt |
| `SHERPA_BARGE_IN_TONE_S`               | `1.0`             | Phase 2 tone duration                      |
| `SHERPA_BARGE_IN_MAX_RATIO`            | `0.65`            | Phase 3 max `cutMs / fullMs`               |
| `SHERPA_BARGE_IN_MIN_FULL_AFTER_NOISE` | `0.75`            | Phase 2 min `cutMs / fullMs`               |
| `SHERPA_BARGE_IN_VERBOSE`              | off               | Log listener speech events                 |

Success: `Semantic barge-in E2E OK — tone ignored, spoken phrase interrupted agent TTS.`

## What each roundtrip catches (confidence matrix)

Passing **unit tests alone** (`npm run test:roundtrip-counting`) does **not** run Sherpa — it only checks evaluators. For release confidence, run **native build + these E2E scripts** (with models):

| Script                                    | Catches                                                                                   | Does **not** catch                                                            |
| ----------------------------------------- | ----------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `start:roundtrip-counting`                | One long utterance → **1** final; **lifecycle open/close**; no mid-count duplicate finals | Second phrase after pause; `speaking_end` without final; multi-client handler |
| `start:roundtrip-utterance-timing`        | `user_speaking_end` → `user_speech_final` within 500 ms; **lifecycle close**              | Dropped first turn when user speaks again; 2× finals                          |
| `start:roundtrip-two-phrases`             | **2×** `user_speech_final`; **2×** `vad_triggered` open sequences                         | Agent TTS echo; `voice-handler` ignore during `agent_speaking`                |
| `start:roundtrip-counting-echo`           | Bidirectional “You said” after **one** counting round                                     | Pending finalize cleared on new `SpeechStart` (fixed in Rust)                 |
| `start:roundtrip-counting-barge-recovery` | Barge truncates echo; recovery echo works; **lifecycle** on listen legs                   | Browser UI timing                                                             |
| `start:roundtrip-barge-in`                | Tone no barge (C1 ok); spoken phrase → **vad_triggered → partial → barge_in**             | Instant VAD barge; partial after agent end                                    |

**Why the bugs you saw slipped through:** earlier tests waited for **one** final per run and never simulated **phrase 1 → long pause → phrase 2** on the same listener. `SpeechStart` cleared `stt_finalize_pending`, so turn 1 never finalized until turn 2 merged in Sherpa.

**Recommended local check (matches CI):**

```bash
cd node-webrtc-rust
npm run build:native
bash scripts/ci/run-sherpa-example-ci.sh vitest   # evaluators only
bash scripts/ci/run-sherpa-example-ci.sh e2e      # all seven start:roundtrip-* scripts
# or: bash scripts/ci/run-pr-tests-full.sh        # full PR quality + integration
```

## CI (GitHub Actions)

Sherpa roundtrips run on every PR and on `main` when the **Test** job executes [`run-pr-integration.sh`](../../scripts/ci/run-pr-integration.sh).

| Job         | Script                                                                                                                                                   | Sherpa roundtrip coverage                                                                      |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| **Quality** | [`run-pr-quality.sh`](../../scripts/ci/run-pr-quality.sh) → [`run-sherpa-example-ci.sh typecheck`](../../scripts/ci/run-sherpa-example-ci.sh) + `vitest` | Typecheck + **Vitest evaluators** (`test:roundtrip-counting`) — no model download              |
| **Test**    | [`run-pr-integration.sh`](../../scripts/ci/run-pr-integration.sh) → [`run-sherpa-example-ci.sh e2e`](../../scripts/ci/run-sherpa-example-ci.sh)          | Downloads EN Kroko STT + Piper TTS, then runs **all** `start:roundtrip*` entry points in order |

E2E order in CI (same as [`run-sherpa-example-ci.sh`](../../scripts/ci/run-sherpa-example-ci.sh)): counting → utterance-timing → two-phrases → **barge-in** → counting-echo → counting-barge-recovery → batch roundtrip.

Path filter: changes under `examples/**` trigger the **examples** filter and run quality + test when other filters also match — see [`scripts/ci/README.md`](../../scripts/ci/README.md#sherpa-roundtrip-e2e-integration-job).

## Debug logging (E2E failures)

Every `start:roundtrip*` script calls `installRoundtripWallClockTimeout()` at startup.

| Context                                                                                | `[speech]` events                      | `[voice-debug]` / topology                                        |
| -------------------------------------------------------------------------------------- | -------------------------------------- | ----------------------------------------------------------------- |
| **Local** (`npm run start:roundtrip*`)                                                 | **On** (browser `speech_event` parity) | **On** by default                                                 |
| **CI** ([`run-sherpa-roundtrip-e2e.sh`](../../scripts/ci/run-sherpa-roundtrip-e2e.sh)) | **On** (streamed to CI log)            | **Off** on first pass; **re-run with `VOICE_DEBUG=1`** on failure |

| Output                                                          | Env                                  | Meaning                                                                                            |
| --------------------------------------------------------------- | ------------------------------------ | -------------------------------------------------------------------------------------------------- |
| `[ci-step] START/OK/FAIL (N/M)`                                 | CI scripts                           | Which integration step is running (see `scripts/ci/ci-step.sh`)                                    |
| `[topology] [signaling\|agent-pc\|user-pc\|listener\|user-sim]` | default on locally; `=0` in CI quiet | Loopback attach + ICE — [`roundtrip-topology-log.ts`](./src/roundtrip-topology-log.ts)             |
| `[e2e-phase]`                                                   | default on                           | Phase boundaries in multi-phase scripts (e.g. barge-in)                                            |
| `[voice-debug]` on **stderr**                                   | `VOICE_DEBUG=1`                      | Rust VAD/STT/gate-hold in `crates/speech`                                                          |
| `[speech] [Phase N] +Nms <event>` on **stderr**                 | `SHERPA_COUNTING_VERBOSE=1`          | Every speech event (like multi-client `speech_event` → browser log)                                |
| Structured failure dump                                         | on `exit 1`                          | Leg stats, finals, re-run hints — [`roundtrip-failure-debug.ts`](./src/roundtrip-failure-debug.ts) |

Opt out of speech events: `SHERPA_ROUNDTRIP_EVENT_LOG=0`. Opt out of rust debug locally: `VOICE_DEBUG=0` or `SHERPA_ROUNDTRIP_DEBUG=0`; topology banners `SHERPA_ROUNDTRIP_TOPOLOGY_LOG=0`. Wall-clock cap: `SHERPA_ROUNDTRIP_WALL_MS` (invalid/zero values fall back to per-script default). **Local CI parity:** `npm run ci:verify:pr-full` (host) or `npm run ci:verify:pr-test:docker` (optional Docker) — see [`scripts/ci/README.md`](../../scripts/ci/README.md).

## Related docs

- [`src/roundtrip-stt-lifecycle-helpers.ts`](./src/roundtrip-stt-lifecycle-helpers.ts) — shared VAD/STT lifecycle evaluators (Vitest)
- [`src/roundtrip-counting.ts`](./src/roundtrip-counting.ts) — shared harness: `AgentSpeakingEndLatch`, `startSpeakerSpeechPump`, `playSpeakerTtsWithPostSilence`, timing helpers
- [`packages/sdk/VOICE-API.md`](../../packages/sdk/VOICE-API.md) — SDK voice exports and speech events
- [`packages/sdk/VOICE-VAD-AND-BARGE-IN.md`](../../packages/sdk/VOICE-VAD-AND-BARGE-IN.md) — VAD/barge-in use cases and defaults
- [Example README](./README.md) — browser demo, model download
- [`crates/vendor-sherpa-onnx/README.md`](../../crates/vendor-sherpa-onnx/README.md) — model layout
- [`packages/sdk/README.md`](../../packages/sdk/README.md) — `VoiceAgent` / VAD config reference
