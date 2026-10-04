# Voice API reference (`@node-webrtc-rust/sdk/voice`)

TypeScript surface for the Rust speech pipeline (`node-webrtc-rust-speech`). For tuning VAD, barge-in, and `gateStt`, see [VOICE-VAD-AND-BARGE-IN.md](./VOICE-VAD-AND-BARGE-IN.md). For Sherpa E2E harness timing (`AgentSpeakingEndLatch`), see [examples/voice-agent-local-sherpa/ROUNDTRIP.md](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md#harness-playback-timing-agentspeakingendlatch).

## Import

```typescript
import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  DEFAULT_VOICE_AGENT_VAD,
  SPEECH_EVENT_TYPE,
  wireVoiceAgentToDataChannel,
  forwardVoiceAgentSpeechToDataChannel,
} from '@node-webrtc-rust/sdk/voice'
```

## `VoiceAgent`

One instance per WebRTC conversation (one inbound + one outbound audio track).

| Method                                    | Description                                                                                                                                 |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `constructor(config?)`                    | Builds native agent; optional `VoiceAgentConfig`.                                                                                           |
| `attach({ inboundTrack, outboundTrack })` | Binds `RemoteAudioTrack` (user mic) and `LocalAudioTrack` (agent TTS out).                                                                  |
| `start()`                                 | Starts STT vendor, TTS drain worker, and inbound `readSample` → `processInboundPcm` loop.                                                   |
| `stop()`                                  | Stops STT and inbound loop.                                                                                                                 |
| `sendTextToTTS(text)`                     | Synthesizes and enqueues PCM on outbound track (20 ms frames).                                                                              |
| `flushTts()`                              | Clears pending TTS (manual barge / cancel).                                                                                                 |
| `waitTtsPlaybackIdle()`                   | Blocks until outbound queue drained and `agent_speaking` false (prefer events in app code).                                                 |
| `on(event, listener)`                     | Subscribe: event name or `'speech'` for all types.                                                                                          |
| `off(event, listener)`                    | Unsubscribe.                                                                                                                                |
| `speechEvents()`                          | Async iterator (`events.mode: 'stream'` or `'both'`). **Agent TTS events only on this agent’s stream** — not on the remote peer’s listener. |
| `updateStt(config)`                     | Queue a new STT vendor config; applies at utterance boundary (or immediately when idle). See [Mid-session language / model switch](#mid-session-stttts-language-and-model-switch). |
| `updateTts(config, options?)`           | Queue a new TTS vendor config before the next `sendTextToTTS` job; optional `cancelInflight`. Same section. |
| `replayLastUtterance()`                 | Re-decode the last finalized utterance with the current STT after a swap. Same section. Max age: `config.replay.maxAgeMs` (default 10 s, cap 120 s). |
| `beginSttHold({ mode, maxBufferMs? })`  | Stop feeding/polling the current STT and buffer user audio while a slow STT swap (cold pool) completes. See [STT hold](#stt-hold-slow-stt-swap). |
| `releaseSttHold({ replay })`            | After `updateStt`: decode the held audio through the new STT (`replay: true`) or drop it. Same section. |
| `cancelSttHold()`                       | Switch failed: resume the old STT, feeding it the buffered audio. Same section. |

### Lifecycle

```text
new VoiceAgent(config)
  → attach(inbound, outbound)
  → start()
  → [ readSample loop → processInboundPcm ]
  → sendTextToTTS / flushTts
  → stop()
```

## Speech events

| `SpeechEventType`      | When emitted                                 | Typical use                                                        |
| ---------------------- | -------------------------------------------- | ------------------------------------------------------------------ |
| `user_speaking_start`  | VAD `SpeechStart`                            | UI “listening” indicator                                           |
| `user_speaking_end`    | End of user turn — see **gateStt** below     | End-of-utterance hint (not always first silence gap)               |
| `user_speech_partial`  | STT streaming                                | Live captions, semantic barge-in                                   |
| `user_speech_final`    | STT utterance closed                         | **Primary LLM turn trigger**                                       |
| `agent_speaking_start` | First TTS PCM written                        | UI “agent talking”                                                 |
| `agent_speaking_end`   | TTS queue drained                            | Harness playback boundary; do not assume remote peer receives this |
| `vad_triggered`        | VAD `SpeechStart` when `vad.enabled`         | STT listen opens; logging / `[speech]` traces                      |
| `stt_stream_start`     | STT vendor PCM feed opened for an utterance  | Pairs with `stt_stream_end`                                        |
| `stt_stream_end`       | STT vendor PCM feed closed                   | After final, C1, or C2 close                                       |
| `user_stt_start`       | STT recognition session opened               | Pairs with `user_stt_end` or `user_stt_not_found`                  |
| `user_stt_end`         | STT recognition session closed               | Normal or forced utterance close                                   |
| `user_stt_not_found`   | VAD fired but no partial within C1 timeout   | No `user_speech_final` — nothing to reply to                       |
| `barge_in`             | Barge-in path fired (VAD and/or STT partial) | Cancel LLM stream; TTS may already be flushed                      |
| `error`                | Vendor or pipeline failure                   | Log / recover                                                      |
| `user_language`        | LID (optional `languageId`)                  | **Detection only** — does not change STT/TTS; your app calls `updateStt` / `updateTts` if needed |
| `stt_config_updated`   | After pending STT config is applied          | Confirm `language`, `modelPath`, `endpoint` on the active recognizer |
| `tts_config_updated`   | After pending TTS config is applied            | Confirm `voice`, `modelPath`, `endpoint` on the active synthesizer |
| `stt_hold_started`     | After `beginSttHold` accepted                  | `holdMode`, `bufferedMs` (audio of the open utterance already held); no transcript text |
| `stt_hold_ended`       | After `releaseSttHold` / `cancelSttHold`       | `holdOutcome` (`released_replay` \| `released_drop` \| `cancelled` \| `failed`), `bufferedMs`, `droppedMs`; no transcript text |
| `voice_language_switching` | Host coordinator (optional)              | UI “switching language…” — **not** emitted by `VoiceAgent` alone; typed for forwarded `speech_event` payloads |
| `voice_language_changed`   | Host coordinator (optional)              | Switch succeeded (host metadata in `text` / `language`) |
| `voice_language_switch_failed` | Host coordinator (optional)            | Switch rejected or vendor error (`error` may be set) |

**Utterance correlation:** `user_speaking_start`, `user_speech_partial`, `user_speech_final`, `user_speaking_end`, and `user_language` on the same turn share `utteranceId` when the native pipeline assigns one. Replay finals set `replay: true` and `replacesUtteranceId` to the original id.

Use `SPEECH_EVENT_TYPE` constants instead of string literals in tests.

### STT utterance lifecycle (event order)

When `vad.enabled` and STT are configured, each VAD `SpeechStart` opens a session:

```text
vad_triggered → user_stt_start → stt_stream_start → user_speaking_start
  → user_speech_partial* → [barge_in if agent TTS + barge config]
  → stt_stream_end → user_stt_end → user_speaking_end → user_speech_final
```

**C1 (no partial):** C1 fires after `sttListenTimeoutMs` of decoder-observed silence (no partial while the recognizer is caught up); a hard cap `sttListenHardTimeoutMs` (default 3×) bounds the wait when the shared decoder is saturated → `stt_stream_end` → `user_stt_not_found` → `user_stt_end` — **no** `user_speech_final`.

**C2 (stall):** after `utteranceFinalizeTimeoutMs` (starts when `sttGateHoldMs` drains if gate was open) → forced close with `user_speech_final` from last partial.

Full flows, timers, and barge matrix: [VOICE-VAD-AND-BARGE-IN.md § STT utterance lifecycle](./VOICE-VAD-AND-BARGE-IN.md#stt-utterance-lifecycle-vad--stt-events). Sherpa harness evaluators: [ROUNDTRIP.md § STT lifecycle evaluators](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md#stt-lifecycle-evaluators).

### `gateStt` and `user_speaking_end`

With **`gateStt: true`** (`VOICE_AGENT_VAD_PRESET`):

1. STT receives audio only while the gate is open (speech, pending, post-speech hold, or utterance closing).
2. After VAD `SpeechEnd`, **`sttGateHoldMs`** keeps the gate open for word gaps.
3. When hold expires and VAD is idle, Rust pushes an **endpoint tail** (400–600 ms synthetic silence) and **`finalize_utterance`**.
4. **`user_speaking_end`** is emitted **paired with** `user_speech_final` (not on the first short pause mid-phrase).

Without STT, `user_speaking_end` may follow gate hold only.

## Mid-session STT/TTS language and model switch

Change recognition or synthesis **without** tearing down the `RTCPeerConnection` or calling `attach()` again. The same `VoiceAgent` instance keeps the inbound `readSample` loop and outbound TTS queue; only the Rust STT/TTS vendor instances are recreated from your new config.

**Application policy:** the library does **not** automatically swap STT/TTS when `user_language` fires. Spoken language ID (LID) is an optional hint event. Whether you switch on LID, on a UI locale picker, or on LLM-detected language is entirely up to your host code (or a session coordinator above `VoiceAgent`). A deployed voice product may opt into LID-driven auto-switch in **that** layer — that is not always-on behavior inside `@node-webrtc-rust/sdk`.

### Typical coordinator flow

```text
user_speech_final (or user_language)
  → decide target locale / model catalog ids
  → await agent.updateStt({ … language, modelPath, … })
  → optional: await agent.updateTts({ … voice, modelPath, … }, { cancelInflight: true })
  → optional: await agent.replayLastUtterance()  // re-decode phrase that triggered the switch
  → listen for stt_config_updated / tts_config_updated
  → handle new user_speech_final (check event.replay)
  → run LLM + sendTextToTTS with the updated TTS config
```

Keep the **PeerConnection** and **tracks** stable; only vendor configs change.

### `updateStt(config)`

Queues a full `SttConfig` (same shape as `VoiceAgent` constructor `stt`).

| When | Behavior |
| ---- | -------- |
| **Idle** (no utterance in progress) | Applies immediately: stops the current STT provider, starts the new one, emits `stt_config_updated`. |
| **Mid-utterance** | Config is held until the current utterance finishes (finalize, `user_speech_final`, gate hold drained). Then the swap runs before the next listen window. |

“Utterance in progress” includes open STT stream, gate hold, and endpoint closing — not only visible partials.

After apply, `stt_config_updated` carries:

| Field on `SpeechEvent` | Source |
| ---------------------- | ------ |
| `language`             | `stt.language` |
| `modelPath`            | `stt.modelPath` (e.g. Sherpa ONNX directory) |
| `endpoint`             | Custom STT endpoint when set |

### `updateTts(config, options?)`

Queues a full `TtsConfig`. The swap runs **before the next** `sendTextToTTS` synthesis job (not necessarily before the next inbound utterance).

| Option | Effect |
| ------ | ------ |
| `cancelInflight: false` (default) | Pending synthesis and playback may finish on the old voice; the next phrase uses the new config. |
| `cancelInflight: true` | Cancels in-flight synthesis and calls the same flush path as barge-in before applying the new TTS provider. Use when the user changed language mid-reply and old-locale audio must stop immediately. |

`tts_config_updated` includes `voice`, `modelPath`, and `endpoint` from the applied config.

### `replayLastUtterance()`

After `updateStt`, the utterance that **triggered** the switch may already have been finalized with the **previous** recognizer. `replayLastUtterance()` re-feeds that turn’s **post-RNNoise** PCM (from an internal ring buffer) through the **current** STT, then emits a new `user_speech_final` with:

- `replay: true`
- `replacesUtteranceId` — id of the original final
- fresh `utteranceId` for the replay pass

Call **only when** no utterance is in progress (after the original final landed). Common pattern: on `user_language` or first final in the wrong locale → `updateStt` → `replayLastUtterance` → treat the replay final as the LLM turn input.

**Failures** (rejected promise / `error` event depending on binding):

| Condition | Meaning |
| --------- | ------- |
| No snapshot yet | No finalized utterance buffered (demo: [`start:replay-last-utterance`](../../examples/voice-agent-local-sherpa-multi-client/README.md)) |
| Utterance still open | Wait for `user_speech_final` before replay |
| Buffer overflow / empty / too old | PCM was not retained — user must speak again |

Full PCM + replay final behavior: Rust `cargo test -p node-webrtc-rust-speech replay_last_utterance`.

**Replay window:** `replayLastUtterance()` refuses audio older than `replay.maxAgeMs` (constructor config, default 10000 ms, capped at 120000 ms; the PCM ring itself keeps at most 15 s per utterance). Raise it only when the host knows the swap can take longer; for swaps that take seconds use the STT hold below, which also covers speech that continues during the wait.

### STT hold (slow STT swap)

A cold STT pool can take 4–45 s to come up. During that time the old recognizer must not emit finals (they would be in the wrong language), and the user keeps talking. `beginSttHold` / `releaseSttHold` / `cancelSttHold` give the host that window.

```text
LID says "de" (or the agent asks for German)
  → await agent.beginSttHold({ mode: 'buffer_replay' })      // stt_hold_started
  → (host plays a wait message, starts the cold pool …)
  → await agent.updateStt({ … German … })                     // pool ready
  → await agent.releaseSttHold({ replay: true })              // held audio → new STT
       → user_speech_final { replay: true, replacesUtteranceId }   then stt_hold_ended
  // or, when the pool never came up:
  → await agent.cancelSttHold()                               // old STT gets the audio back
```

| Behavior | Detail |
| -------- | ------ |
| After `beginSttHold` resolves | The current STT is not fed and not polled: **no** `user_speech_partial` / `user_speech_final` from the old model is emitted. VAD and `user_speaking_*` / `vad_triggered` / `stt_stream_*` / `user_stt_*` events keep flowing; STT-partial semantic barge-in is inactive (VAD barge-in still works). C1/C2 forced finals are suppressed. |
| Where audio is held | In the native agent, in the same post-RNNoise mono 16 kHz stream that would have gone to STT (including the SpeechStart pre-roll). Silence between utterances is not stored. The utterance in progress is seeded from the utterance replay ring (up to 15 s). |
| `buffer_replay` | Keeps everything until release, bounded by `maxBufferMs` (default 45000, cap 120000; values above the cap are clamped, `0` is invalid). When full, the oldest audio is dropped and counted in `droppedMs`. |
| `first_utterance` | Keeps only the triggering utterance (the one open at `beginSttHold`, else the first one that starts). After it ends, later speech is dropped and counted in `droppedMs`. |
| `releaseSttHold({ replay: true })` | Applies a still-queued `updateStt` config, then feeds the held audio to the new STT in order and finalizes it, emitting `user_speech_final` with `replay: true` (`replacesUtteranceId` = the original utterance when one existed). While the held audio is fed, live audio keeps being diverted into the same buffer; the hold is cleared atomically once the buffer is empty, so the order is held audio, then live audio, with no gap and no duplicate. If an utterance is still open at that moment, its final comes from the live path and carries `replay: true`. |
| `releaseSttHold({ replay: false })` | Drops the buffer; the new STT only sees audio from now on. |
| `cancelSttHold()` | Discards a queued `updateStt`, feeds the buffered audio to the old STT in order, and closes utterances that ended during the hold through the normal path (normal finals, `replay` not set). |
| Errors | `beginSttHold` rejects if a hold is active or the agent is not running; `release`/`cancel` reject without an active hold. `replayLastUtterance()` rejects while held. A feed or finalize failure ends the hold with `holdOutcome: 'failed'` and rejects. The snapshot used by `replayLastUtterance()` is invalidated by a hold (speak again). |
| Logging / events | `stt_hold_started` and `stt_hold_ended` carry counts only (`bufferedMs`, `droppedMs`, `holdMode`, `holdOutcome`), never transcript text. |

`stop()` clears an active hold. Mock-vendor coverage: `cargo test -p node-webrtc-rust-speech --test stt_hold_test`.

### Host-level speech events (`voice_language_*`)

`SpeechEventType` includes `voice_language_switching`, `voice_language_changed`, and `voice_language_switch_failed` so **session coordinators** can mirror switch lifecycle on the same `speech_event` wire as STT/TTS events (for example when forwarding to a browser DataChannel). The native `VoiceAgent` pipeline emits `stt_config_updated` / `tts_config_updated`; the `voice_language_*` kinds are for **your** coordinator to emit when it orchestrates catalog lookups, secrets, and `updateStt` / `updateTts` as one logical “switch.”

### LID vs mid-session swap

| Mechanism | What it does |
| --------- | ------------ |
| `languageId` + `user_language` | Offline identify → ISO 639-1 hint on the utterance (`utteranceId` when assigned). |
| `updateStt` / `updateTts` | Actually changes vendors/models/voices for subsequent audio. |
| Auto-switch on LID | **Not built into the SDK** — implement in app code (or a host worker) if you want it. |

**LID model residency:** the Whisper tiny identifier is loaded once per process and shared by every `VoiceAgent` with the same `languageId.modelPath` (inference calls serialise on one lock; each call uses its own stream). Constructing a `VoiceAgent` starts that load in the background. A host that starts before any session exists (a runner pod) should await `preloadLanguageId({ modelPath })` from `@node-webrtc-rust/sdk/voice` at boot so the first utterance never pays the load. Set `VOICE_DEBUG=1` to see `LID model load` and `LID identify` timings (no transcript text).

See [README § Spoken language identification](../../README.md#spoken-language-identification) for LID setup; see [packages/sdk/README.md](./README.md#mid-session-stttts-language-switch) for a Node-oriented summary.

### Headless SDK check (no Sherpa models)

```bash
npm run start:replay-last-utterance --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
```

Uses `mock` vendors to exercise `updateStt` and the error path when replay has no buffered utterance.

## Presets

| Export                    | `gateStt` | Use                                                    |
| ------------------------- | --------- | ------------------------------------------------------ |
| `DEFAULT_VOICE_AGENT_VAD` | `false`   | Matches Rust `VadConfig::default()` when `vad` omitted |
| `VOICE_AGENT_VAD_PRESET`  | `true`    | **Recommended** for production voice bots              |

Both include semantic barge-in defaults (`requireSttPartial: true`).

## Data channel bridge

| Export                                                 | Role                                                                     |
| ------------------------------------------------------ | ------------------------------------------------------------------------ |
| `VOICE_CONTROL_CHANNEL_LABEL`                          | Recommended label: `'voice-control'`                                     |
| `wireVoiceAgentToDataChannel(agent, channel)`          | Inbound `{ type: 'speak', text }` → `sendTextToTTS`                      |
| `forwardVoiceAgentSpeechToDataChannel(agent, channel)` | `speechEvents()` → JSON `speech_event` on channel (call after `start()`) |
| `parseVoiceControlClientMessage(raw)`                  | Parse client JSON                                                        |
| `speechEventToControlMessage(event)`                   | Serialize for wire                                                       |

## Debug

| Export / env                 | Effect                                      |
| ---------------------------- | ------------------------------------------- |
| `isVoiceDebugEnabled()`      | `VOICE_DEBUG=1`                             |
| `voiceDebugLog(module, msg)` | stderr `[voice-debug]` from TS              |
| `VOICE_DEBUG` (Rust)         | stderr `[voice-debug]` from native pipeline |

## Rust crate (`node-webrtc-rust-speech`)

Public Rust API mirrors the NAPI config types:

| Module         | Contents                                                    |
| -------------- | ----------------------------------------------------------- |
| `agent`        | `VoiceAgent` — `process_inbound_pcm`, TTS/STT orchestration |
| `config`       | `VadConfig`, `BargeInConfig`, `VoiceAgentConfig`, vendors   |
| `events`       | `SpeechEvent`, `SpeechEventKind`, `SpeechEventBus`          |
| `vad`          | `VadEngine`, `VadTransition`, `handle_barge_in`             |
| `stt_pre_roll` | Pre-roll ring when `gate_stt` + VAD enabled                 |
| `pipeline`     | `SttProvider`, `TtsProvider` traits                         |
| `pcm`          | `stereo_48k_to_mono_16k`, sample-rate helpers               |

NAPI bindings in `@node-webrtc-rust/bindings` expose `JsVoiceAgent` to Node; application code should use this SDK package.

## Vendor streaming semantics

VoiceAgent always emits the same speech events; vendors differ in **when** partial text and TTS PCM arrive.

### STT: partials vs finalize-only

| Pattern | `user_speech_partial` | Examples |
| ------- | --------------------- | -------- |
| **Live WebSocket** with documented interim results | Yes, while the user talks | Deepgram `nova-*` listen, ElevenLabs Scribe, AssemblyAI v3, Google Chirp V2 streaming, AWS Transcribe streaming, Sherpa, OpenAI Realtime |
| **File / REST on utterance end** | No until finalize (then one final) | Groq Whisper multipart, Azure REST short audio, Google `latest_long` REST, OpenAI file JSON (`whisper-1`) |
| **File SSE / committed Realtime** | Text deltas per vendor rules | OpenAI file SSE, Realtime committed-turn models |

`poll_transcript` in Rust vendors blocks until an in-flight file POST or WS commit finishes so `finalize_utterance` can return without stalling the inbound PCM loop.

**Documented gaps:** Groq has no live listen socket; Azure REST returns finals only (Voice Live not in `vendor-azure`); Deepgram **Flux listen** is not in the documented listen matrix — use `nova-2` / `nova-3`.

### TTS: progressive vs full-body

| Pattern | Playback | Examples |
| ------- | -------- | -------- |
| **Progressive** | PCM chunks before the HTTP/WS response completes | OpenAI chunked PCM, Deepgram Aura/Flux speak WS, ElevenLabs HTTP `/stream`, Google `StreamingSynthesize`, Cartesia WS |
| **Full-body then frame** | Entire audio buffered, then 20 ms outbound frames | Groq Orpheus WAV, Azure REST, Polly `SynthesizeSpeech`, ElevenLabs `eleven_v3`, many fallbacks |

`sendTextToTTS` enqueues on the outbound track at 20 ms cadence regardless; progressive vendors start playback sooner.

Full transport × model tables: [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../../examples/shared/VOICE_VENDOR_REFERENCE.md#streaming-transport-matrix-voiceagent). Per-crate matrices: `crates/vendor-*/src/matrix.rs`.

## Related

- [README.md](./README.md) — quick start, Pipeline B, [mid-session language switch](./README.md#mid-session-stttts-language-switch)
- [VOICE-VAD-AND-BARGE-IN.md](./VOICE-VAD-AND-BARGE-IN.md) — tuning guide
- [crates/speech/src/lib.rs](../../crates/speech/src/lib.rs) — rustdoc entry point
