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
| `barge_in`             | Barge-in path fired (VAD and/or STT partial) | Cancel LLM stream; TTS may already be flushed                      |
| `error`                | Vendor or pipeline failure                   | Log / recover                                                      |

Use `SPEECH_EVENT_TYPE` constants instead of string literals in tests.

### `gateStt` and `user_speaking_end`

With **`gateStt: true`** (`VOICE_AGENT_VAD_PRESET`):

1. STT receives audio only while the gate is open (speech, pending, post-speech hold, or utterance closing).
2. After VAD `SpeechEnd`, **`sttGateHoldMs`** keeps the gate open for word gaps.
3. When hold expires and VAD is idle, Rust pushes an **endpoint tail** (400–600 ms synthetic silence) and **`finalize_utterance`**.
4. **`user_speaking_end`** is emitted **paired with** `user_speech_final` (not on the first short pause mid-phrase).

Without STT, `user_speaking_end` may follow gate hold only.

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

## Related

- [README.md](./README.md) — quick start and Pipeline B
- [VOICE-VAD-AND-BARGE-IN.md](./VOICE-VAD-AND-BARGE-IN.md) — tuning guide
- [crates/speech/src/lib.rs](../../crates/speech/src/lib.rs) — rustdoc entry point
