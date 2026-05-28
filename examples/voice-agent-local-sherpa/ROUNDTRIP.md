# Sherpa TTS → STT roundtrip

Node integration test for on-device Sherpa speech: **text → TTS → WebRTC → STT → text**, with VAD and `gateStt` enabled on the listener.

Implementation: [`src/roundtrip.ts`](./src/roundtrip.ts).

## Can gaps come from our VAD?

**Yes — for timing inside and after an utterance.** **No — `minSilenceDurationMs` is not a “pause between batch phrases” knob.**

| Source | What it does |
|--------|----------------|
| **`minSilenceDurationMs` (300 ms default)** | How long silence must last **during** one utterance before VAD emits `user_speaking_end`. Short Piper gaps between words should stay under this so one TTS phrase stays one segment. |
| **`sttGateHoldMs` (2500 ms default)** | After `user_speaking_end`, keep feeding STT for this much **audio duration** (sum of inbound PCM frame lengths). With real-time trailing silence, that aligns with wall-clock time. |
| **Endpoint tail** | `max(minSilenceDurationMs, 800)` ms of silence pushed to STT after hold reaches zero, then `finalize_utterance`. |
| **`speechPadMs` / `minSpeechDurationMs`** | Pre-roll and minimum voiced time before `user_speaking_start` — not inter-phrase gaps. |

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

export SHERPA_STT_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-2023-06-26"
export SHERPA_TTS_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low"

npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

## Run modes

| Mode | Command |
|------|---------|
| **Batch (default)** | `npm run start:roundtrip` — 5 built-in sentences + similarity table |
| **Single phrase** | `npm run start:roundtrip -- "I love America"` |
| **Single via env** | `SHERPA_ROUNDTRIP_PHRASE="Hello world" npm run start:roundtrip` |

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

| Input (normalized) | Recognized (normalized) | Similarity |
|--------------------|-------------------------|------------|
| `i love america` | `i love america` | 100% |
| `i love america` | `thy love america` | 67% (fails at 75% threshold) |
| `the weather is nice today` | `the weather weather is nice to day` | 80% |

Exact character match is not required; STT may split or repeat words (`to day` vs `today`).

## Timing: VAD vs explicit silence

Gaps and pauses come from **two layers**: the **VoiceAgent VAD/STT pipeline** (listener) and **optional explicit PCM** on the speaker outbound track (test harness only).

### Within one phrase (listener VAD + gateStt)

| Setting | Default | Role |
|---------|---------|------|
| `minSpeechDurationMs` | 250 | Voice must be present this long before `user_speaking_start` |
| `minSilenceDurationMs` | 300 | Silence this long ends the utterance (`user_speaking_end`) — avoids splitting on short TTS word gaps |
| `speechPadMs` | 300 | Pre-roll ring size only (`speechPadMs + minSpeechDurationMs` ≈ 550 ms buffered before `SpeechStart`) |
| `gateStt` | true | STT only while gate is open |
| `gateSttOpenOnPending` | true | Gate opens during VAD **pending** speech (before `SpeechStart`) — covers WebRTC lead-in |
| `sttGateHoldMs` | 2500 | After `SpeechEnd`, keep feeding STT for this many ms (trailing phonemes + relay) |
| Endpoint tail | max(`minSilenceDurationMs`, 800) | Extra silence pushed to STT after hold expires, then `finalize_utterance` |

**`minSilenceDurationMs` is not a gap between batch phrases.** It only controls how long silence must last **inside** an utterance before VAD declares speech ended. Piper TTS short pauses between words should stay below 300 ms so one phrase stays one segment.

### After each phrase (listener finalize path)

When the listener’s VAD fires `user_speaking_end`:

1. `sttGateHoldMs` counts down on incoming PCM frames (needs real audio/silence on the wire).
2. Hold expiry → endpoint tail silence → Sherpa `finalize_utterance` → `user_speech_final`.

The roundtrip waits for **`user_speech_final`** (or timeout / partial fallback) before starting the next phrase. That wait is the main **logical** separator between batch sentences.

### Trailing silence after TTS (speaker outbound, VAD-aligned)

After each `sendTextToTTS`, the harness streams trailing silence on the speaker outbound track **at real time** (in parallel with waiting for `user_speech_final`):

```text
postTtsSilenceS = (sttGateHoldMs + endpointTailMs + margin) / 1000
```

Defaults ≈ **3.8 s wall time** (2500 + 800 + 500 ms of 20 ms frames). Duration is **derived from listener VAD config** so hold and endpoint tail see silence paced like a quiet mic after the user stops talking.

### Between phrases (batch)

| Mechanism | Default | Purpose |
|-----------|---------|---------|
| Wait for `user_speech_final` | always | Next TTS starts only after previous utterance finalized |
| Trailing silence (above) | ~3.8 s from VAD timings | Lets hold + finalize complete on the listener |
| `SHERPA_ROUNDTRIP_GAP_S` | **0** | Extra explicit silence between phrases; **off by default** |

With `SHERPA_ROUNDTRIP_GAP_S=0` (default), **inter-phrase gaps come from VAD-driven finalize timing plus VAD-aligned trailing silence**, not from a separate fixed 1 s gap. The harness **must** stream that trailing PCM at real time (in parallel with waiting for `user_speech_final`) so `sttGateHoldMs` can count down on the wire; without it, the next phrase can start before finalize and STT can bleed across phrases.

Set `SHERPA_ROUNDTRIP_GAP_S=1` (or higher) only if you need extra separation beyond what hold + trailing silence provide.

### Before the first phrase

| Mechanism | Default | Purpose |
|-----------|---------|---------|
| `SHERPA_ROUNDTRIP_WARMUP_S` | 0.6 s | Explicit silence on speaker outbound to prime WebRTC before first TTS (not VAD) |

## Environment variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `SHERPA_ROUNDTRIP_PHRASE` | — | Single phrase (skips 5-sentence batch) |
| `SHERPA_ROUNDTRIP_MIN_SIMILARITY` | `0.75` | Min word-match ratio to pass |
| `SHERPA_ROUNDTRIP_TIMEOUT_MS` | `45000` | Per-phrase STT timeout |
| `SHERPA_ROUNDTRIP_WARMUP_S` | `0.6` | Speaker warmup silence (seconds) before first TTS |
| `SHERPA_ROUNDTRIP_GAP_S` | `0` | **Extra** silence between phrases (seconds); VAD trailing usually enough |
| `SHERPA_ROUNDTRIP_VERBOSE` | unset | Set to `1` for per-frame VAD/STT logs |

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
  sttGateHoldMs: 2500,
  bargeIn: { enabled: true, flushTts: true },
}
```

`minSilenceDurationMs` is omitted → library default **300 ms** (see `VadConfig` in `crates/speech`).

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

## Barge-in E2E

[`src/roundtrip-barge-in.ts`](./src/roundtrip-barge-in.ts) — same WebRTC loopback as the STT roundtrip, but tests **interrupting agent TTS** mid-utterance.

### What barge-in is

**Barge-in** = the **speaking agent** stops its TTS and emits `barge_in`.

```typescript
bargeIn: { enabled: true, useVad: true, flushTts: true }
```

| Setting | Behavior |
|---------|----------|
| `enabled` | Master switch for flush + `barge_in`. |
| `useVad: true` (default) | Automatic interrupt on inbound VAD `SpeechStart` (`vad.enabled` required). |
| `useVad: false` | No auto interrupt on voice; call `flushTts()` from your app only. |

Avoid false interrupts from short tones/noise when `useVad: true` by raising `vad.minSpeechDurationMs` (e.g. 200–300 ms) or `vad.threshold` on the **speaker** agent.

| Requirement | Why |
|-------------|-----|
| `vad.enabled: true` on the **speaker** | Needed for `useVad` auto barge-in |
| `vad.bargeIn.useVad: true` | E2E test uses this; tone on `agentInbound` triggers `SpeechStart` |
| Interrupt audio on speaker **inbound** (`agentInbound`) | User leg tone simulates the user talking over the agent |

```text
Speaker (agent PC)                    User leg
Sherpa TTS → agentOut → userInbound   userOut → agentInbound (440 Hz tone)
VAD on agentInbound                   voice activity → SpeechStart → barge_in + flush TTS
```

The STT roundtrip **listener** has VAD for `gateStt` but does not play TTS — barge-in there would not cut the speaker’s playback. This test puts VAD + barge-in on the **speaker**.

| Phase | What happens |
|-------|----------------|
| 1 | Long phrase, no interrupt → measure full received audio (ms) on `userInbound` |
| 2 | Same phrase; after `SHERPA_BARGE_IN_DELAY_MS`, stream user tone at real time → expect `barge_in` and received audio **&lt; 65%** of phase 1 (configurable) |

```bash
npm run start:roundtrip-barge-in --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

| Variable | Default | Purpose |
|----------|---------|---------|
| `SHERPA_BARGE_IN_PHRASE` | long built-in sentence | TTS text |
| `SHERPA_BARGE_IN_DELAY_MS` | `400` | Wait before injecting user tone (must be before TTS finishes) |
| `SHERPA_BARGE_IN_INTERRUPT_S` | `1.2` | User tone duration (real-time frames) |
| `SHERPA_BARGE_IN_MAX_RATIO` | `0.65` | Max allowed `cutMs / fullMs` |
| `SHERPA_BARGE_IN_VERBOSE` | off | Log speaker speech events |

Success ends with `Barge-in E2E OK — TTS playback was truncated after user interrupt.`

## Related docs

- [`packages/sdk/VOICE-VAD-AND-BARGE-IN.md`](../../packages/sdk/VOICE-VAD-AND-BARGE-IN.md) — VAD/barge-in use cases and defaults
- [Example README](./README.md) — browser demo, model download
- [`crates/vendor-sherpa-onnx/README.md`](../../crates/vendor-sherpa-onnx/README.md) — model layout
- [`packages/sdk/README.md`](../../packages/sdk/README.md) — `VoiceAgent` / VAD config reference
