# VAD and barge-in guide

How `VoiceAgent` uses voice activity detection (VAD) and barge-in, which settings matter, and what you can leave at defaults.

Rust defaults live in `crates/speech/src/config.rs`. TypeScript mirrors them in [`src/voice/defaults.ts`](./src/voice/defaults.ts).

## Philosophy: defaults first

Most phone-bot / voice-assistant apps only need:

```typescript
import { VOICE_AGENT_VAD_PRESET } from '@node-webrtc-rust/sdk/voice'

const agent = new VoiceAgent({
  stt: { provider: 'deepgram', /* … */ },
  tts: { provider: 'openai', /* … */ },
  vad: VOICE_AGENT_VAD_PRESET,
  events: { mode: 'both' },
})
```

Or omit `vad` entirely and get library defaults (VAD on, barge-in on, `gateStt` off). For production voice agents that stream STT, prefer **`VOICE_AGENT_VAD_PRESET`** (`gateStt: true`).

Override a field only when you hit a concrete issue (false barge-in, STT cutting off words, TTS split into two utterances).

---

## Defaults at a glance

| Field | Default | Role |
|-------|---------|------|
| `vad.enabled` | `true` | Inbound VAD on |
| `vad.provider` | `energy` | RMS VAD in default native build |
| `vad.threshold` | `0.15` | Energy RMS default (not Silero 0.5) |
| `vad.minSpeechDurationMs` | `250` | Min voiced time before `user_speaking_start` |
| `vad.minSilenceDurationMs` | `300` | Min silence before `user_speaking_end` (avoids TTS word-gap splits) |
| `vad.speechPadMs` | `300` | Pre-roll ring size for `gateStt` (not subtracted from speech start) |
| `vad.gateStt` | `false` | If `true`, STT only while gate is open |
| `vad.gateSttOpenOnPending` | `true` | Include VAD “pending” speech in gate (WebRTC lead-in) |
| `vad.sttGateHoldMs` | `2500` | Keep feeding STT after `user_speaking_end` |
| `vad.bargeIn.enabled` | `true` | Allow barge-in flush + event |
| `vad.bargeIn.useVad` | `true` | Auto barge on VAD `SpeechStart` |
| `vad.bargeIn.flushTts` | `true` | Clear pending TTS PCM on barge-in |

**Shipped native build** uses **energy VAD** only (`provider: "energy"`). See [VAD providers](#vad-providers-energy-vs-silero) below.

---

## VAD providers (energy vs Silero)

Two backends share the same `VadConfig` timing fields (`minSpeechDurationMs`, `minSilenceDurationMs`, `speechPadMs`, barge-in, `gateStt`). Only **detection** differs.

### How to choose

| `vad.provider` | When to use |
|----------------|-------------|
| **`energy`** (default) | **npm `build:native` / published binaries** — no extra deps, tune `threshold` for your mic/noise floor |
| **`silero`** | Custom native build with `silero-vad` Cargo feature **and** `provider: "silero"` — better speech vs noise, heavier binary |

```typescript
// Shipped binary (energy) — recommended default
vad: { ...VOICE_AGENT_VAD_PRESET, provider: 'energy', threshold: 0.15 }

// Custom Silero build only
vad: { ...VOICE_AGENT_VAD_PRESET, provider: 'silero', threshold: 0.5 }
```

If you set `provider: 'silero'` on the **stock** `.node` without rebuilding, `VoiceAgent` / `attach` fails with a config error (no silent fallback). Use `provider: 'energy'` or rebuild with `silero-vad`.

### Comparison

| | **Energy** | **Silero** |
|---|------------|------------|
| **In shipped `.node`** | Yes (default Cargo feature `energy-vad`) | No |
| **Algorithm** | RMS of mono PCM vs `threshold` | Small ONNX model (~309k params), probability vs `threshold` |
| **Model / runtime size** | None (inline math) | Model ~**1–2 MB** embedded in `silero-vad-rust`; **ONNX Runtime loaded dynamically** at runtime (not in the `.node`) — install ORT separately (see below) |
| **CPU per 20 ms frame** | Negligible (one RMS) | ~**&lt;1 ms** per ~30 ms chunk (upstream Silero docs; plus ORT overhead) |
| **Threshold scale** | RMS ~**0.05–0.2** (Sherpa example uses `0.05`) | Probability ~**0.3–0.6** (default config uses `0.5` when you opt into Silero) |
| **False triggers** | Tones, keyboard, loud noise can look like “speech” | Generally fewer false starts in noise |
| **Tuning** | `threshold`, `minSpeechDurationMs` | Same timing fields + Silero `threshold` |
| **Sample rate** | 8 / 16 kHz via `vad.sampleRate` | 8 / 16 kHz (Silero backend) |

### Weight (how heavy is Silero?)

| Component | Energy | Silero (this repo) |
|-----------|--------|---------------------|
| Extra Rust code | A few KB | `silero-vad-rust` + `ort` crate (dynamic load only) |
| Embedded model | — | ~**1.2–2.2 MB** (v5/v6 ONNX inside `silero-vad-rust`) |
| **`.node` size delta** | **0** | Modest (Rust + model bytes); **ORT is not linked into the addon** |
| **Runtime on disk** | — | **ONNX Runtime** you install yourself (~**15–50 MB** depending on platform/build) |

Published macOS arm64 `.node` today is ~**90 MB** (WebRTC, vendors, Sherpa, etc.). Sherpa already ships **its own** ONNX stack for STT/TTS; Silero VAD uses a **second** ONNX Runtime via `ort` — we do **not** bundle that into npm artifacts.

**Status:** `silero-vad` is an optional feature on `node-webrtc-rust-speech` and `node-webrtc-rust-bindings`. Default npm / CI builds use **energy only**. Silero is **documented, opt-in, not CI-shipped** (ORT + model size).

### Enabling Silero (maintainers / custom builds)

There is **no** runtime toggle: one VAD backend per compiled `.node`.

#### 1. Install ONNX Runtime locally (required)

Silero uses the [`ort`](https://crates.io/crates/ort) crate with **`load-dynamic`**: at runtime the process must find **`libonnxruntime.dylib`** (macOS), **`libonnxruntime.so`** (Linux), or **`onnxruntime.dll`** (Windows). We **do not** download or copy ORT into the published `.node`; custom-build operators install it on each machine / image.

**Version:** speech pins `ort = 2.0.0-rc.10`, which targets **ONNX Runtime 1.22**. Use a 1.22-compatible install when possible (newer minors are often fine; mismatches show up as load or inference errors).

**Not Sherpa’s ONNX:** the default bindings build already includes ONNX inside `sherpa-onnx` for local STT/TTS. That runtime is **not** shared with Silero VAD — you still need a separate ORT install for `provider: 'silero'`.

**macOS (Homebrew example):**

```bash
brew install onnxruntime
export DYLD_LIBRARY_PATH="$(brew --prefix onnxruntime)/lib:${DYLD_LIBRARY_PATH:-}"
```

**Linux:** install from your distro if available, or unpack a [Microsoft ONNX Runtime release](https://github.com/microsoft/onnxruntime/releases) (CPU build is enough for VAD) and point the loader at the `lib/` directory:

```bash
export LD_LIBRARY_PATH="/path/to/onnxruntime/lib:${LD_LIBRARY_PATH:-}"
```

**Windows:** add the directory containing `onnxruntime.dll` to `PATH`.

**Verify before running Node** (optional):

```bash
# macOS — should print a path, not "no such file"
ls "$(brew --prefix onnxruntime)/lib/libonnxruntime.dylib" 2>/dev/null || ls libonnxruntime.dylib
```

If ORT is missing, the first Silero inference typically fails with an error like `dlopen(libonnxruntime.dylib, …): tried: … (no such file)`.

**Deploying custom builds:** document ORT installation for your team (same as above), or bake ORT into your container/AMI. We intentionally avoid bundling ORT in npm to keep default artifacts smaller; static/bundled ORT remains a possible future maintainer choice (`ort` features `download-binaries` + `copy-dylibs`) at the cost of a much larger per-platform binary.

#### 2. Build native with `silero-vad`

Speech pins `ort = 2.0.0-rc.10` for `silero-vad-rust` compatibility:

```bash
cd node-webrtc-rust
npm run build:native -- --features silero-vad
```

Or enable `features = ["silero-vad"]` on the bindings crate in `packages/bindings/Cargo.toml`, then `npm run build:native`.

Run your app with the same `DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH` / `PATH` you used when testing.

#### 3. Configure the agent

```typescript
vad: { ...VOICE_AGENT_VAD_PRESET, provider: 'silero', threshold: 0.5, sampleRate: 16000 }
```

### Which should I use?

| Situation | Recommendation |
|-----------|----------------|
| Default npm package, demos, Sherpa local example | **`energy`** + `VOICE_AGENT_VAD_PRESET`; tune `threshold` (Sherpa uses `0.05`) |
| Noisy office, fan, music bleed, false barge-in | Try higher `minSpeechDurationMs` first; then consider a **Silero custom build** |
| Maximum simplicity, CI, edge devices | **Energy** |
| You control native builds, can install ORT locally, and want best VAD quality | **Silero** custom build |

---

## Use cases

### 1. Standard voice agent (one peer, user talks, agent replies)

**One `VoiceAgent`** on the call:

- `inboundTrack` = user mic (remote)
- `outboundTrack` = agent TTS (local)
- **VAD + barge-in + `gateStt`** on this agent

```typescript
vad: VOICE_AGENT_VAD_PRESET
// or: { gateStt: true }  // everything else default
```

| Piece | Setting | Why |
|-------|---------|-----|
| VAD | `enabled: true` (default) | Utterance boundaries, barge-in |
| `gateStt` | `true` | Don’t stream silence/noise to STT |
| `bargeIn` | defaults (`useVad: true`) | User can talk over TTS |
| `sttGateHoldMs` | default `2500` | Trailing phonemes after user stops |

**App wiring:**

```typescript
agent.on('user_speech_final', (e) => startLLM(e.text!))
agent.on('barge_in', () => cancelLLM())
```

You do **not** need a second agent for barge-in on a normal client.

---

### 2. Listen-only leg (STT roundtrip, conference listener)

Agent **receives** audio but **does not** play TTS on the same instance (or never calls `sendTextToTTS`).

```typescript
vad: {
  gateStt: true,
  bargeIn: { enabled: false }, // optional: no TTS to interrupt
}
```

| Piece | Setting | Why |
|-------|---------|-----|
| `bargeIn.enabled` | `false` | No outbound TTS → barge-in has no effect |
| `gateStt` | `true` | STT only during speech |

Example: [`examples/voice-agent-local-sherpa` roundtrip](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md) — **speaker** has `vad.enabled: false`; **listener** uses `VOICE_AGENT_VAD_PRESET`.

---

### 3. Separate TTS speaker + STT listener (two peers)

Used in tests and some pipelines:

| Peer | VAD | Barge-in | Notes |
|------|-----|----------|-------|
| **Speaker** (plays TTS) | `enabled: true`, `gateStt: false` | `useVad: true` | Inbound = interrupt audio (user leg) |
| **Listener** (STT only) | `gateStt: true` | `enabled: false` | Does **not** stop speaker TTS |

Barge-in only cuts TTS on the agent that **plays** audio and **hears** the interrupt on **inbound**.

---

### 4. Manual interrupt only (no VAD-driven barge)

Push-to-talk, hardware mute, or your own cloud VAD:

```typescript
vad: {
  ...VOICE_AGENT_VAD_PRESET,
  bargeIn: { enabled: true, useVad: false, flushTts: true },
}
```

Call `agent.flushTts()` when **you** decide to interrupt. Inbound tones/noise will **not** auto-cut TTS.

`user_speaking_start` / `end` still fire if VAD stays enabled.

---

### 5. Disable barge-in, keep VAD events

Agent never plays TTS, or you handle overlap in the UI only:

```typescript
vad: {
  gateStt: true,
  bargeIn: { enabled: false },
}
```

---

### 6. Disable VAD entirely

Always-on STT (rare, higher cost):

```typescript
vad: { enabled: false }
```

No `user_speaking_*`, no `barge_in`. STT receives all inbound PCM (vendor permitting).

---

## Barge-in reference

**Barge-in** = stop pending agent TTS and emit `barge_in` so the app can cancel the LLM stream.

```typescript
bargeIn: {
  enabled: true,   // master switch
  useVad: true,    // auto: inbound VAD SpeechStart
  flushTts: true,  // clear queued PCM before event
}
```

| `enabled` | `useVad` | What happens |
|-----------|----------|----------------|
| `false` | — | No `barge_in`, no TTS flush from barge path |
| `true` | `true` | **Automatic** on VAD `SpeechStart` (`vad.enabled` required) |
| `true` | `false` | **Manual** via `flushTts()` only |

Event order on auto barge: optional TTS flush → `barge_in` → `user_speaking_start`.

**Requirements for auto barge:**

1. Same `VoiceAgent` that calls `sendTextToTTS`
2. `vad.enabled: true`
3. Real user speech on `inboundTrack` (not agent TTS loopback on that track)

---

## VAD timing (when to tune)

### `minSpeechDurationMs` (default 250)

Time inbound audio must look “voiced” before `user_speaking_start` / auto barge.

| Symptom | Direction |
|---------|-----------|
| Coughs / clicks trigger barge-in | **Increase** (300–400) |
| User feels lag before agent reacts | **Decrease** (150–200) cautiously |

### `minSilenceDurationMs` (default 300)

Silence needed **inside** one utterance to emit `user_speaking_end`.

| Symptom | Direction |
|---------|-----------|
| One sentence split into two STT finals | **Increase** (400–500) |
| Agent waits too long after user stops | **Decrease** (200–250) |

Keeps short TTS word gaps (&lt; 300 ms) inside a single utterance when the agent is speaking.

### `threshold` (default 0.5)

| Symptom | Direction |
|---------|-----------|
| Noise floor triggers speech | **Increase** |
| Quiet speakers never start | **Decrease** |

### `sttGateHoldMs` (default 2500)

Audio time fed to STT **after** `user_speaking_end`. Usually leave default; see [ROUNDTRIP.md](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md) for harness timing.

### `speechPadMs` (default 300)

Pre-roll buffer capacity for `gateStt` only. Rarely change; does **not** delay `SpeechStart`.

---

## Recommendations (quick)

| Goal | Suggested config |
|------|------------------|
| **Default voice bot** | `vad: VOICE_AGENT_VAD_PRESET` + `on('barge_in')` / `on('user_speech_final')` |
| **Minimal config** | Omit `vad` or `{}` — add `gateStt: true` for real STT |
| **No false interrupts from beeps** | Keep defaults; raise `minSpeechDurationMs` to 300 first |
| **No auto interrupt** | `bargeIn: { useVad: false }` + `flushTts()` |
| **STT-only leg** | `gateStt: true`, `bargeIn.enabled: false` |
| **Local Sherpa** | `VOICE_AGENT_VAD_PRESET` + `threshold: 0.05` for energy VAD on quiet RMS scale |

---

## Local Sherpa (on-device)

[`examples/voice-agent-local-sherpa`](../../examples/voice-agent-local-sherpa/README.md) sets:

- `threshold: 0.05` — energy VAD RMS scale, not Silero 0.5
- Otherwise aligned with `VOICE_AGENT_VAD_PRESET` (250 / 300 ms speech/silence, `gateStt`, barge-in defaults)

Do not copy `0.05` into cloud Silero deployments.

**Tests:**

- TTS → STT: `npm run start:roundtrip`
- Barge-in: `npm run start:roundtrip-barge-in` — [ROUNDTRIP.md § Barge-in E2E](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md#barge-in-e2e)

---

## Related

- [`packages/sdk/README.md`](./README.md) — VoiceAgent API
- [`examples/voice-agent-local-sherpa/ROUNDTRIP.md`](../../examples/voice-agent-local-sherpa/ROUNDTRIP.md) — timing and loopback tests
