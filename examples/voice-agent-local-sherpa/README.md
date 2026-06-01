# Local Sherpa STT + TTS — Browser + Node

**Free, on-device streaming speech-to-text and text-to-speech** with [Sherpa-ONNX](https://github.com/k2-fsa/sherpa-onnx) — no cloud API keys, no per-minute billing.

This example mirrors [`voice-agent-browser`](../voice-agent-browser/README.md) but uses:

| Component | Provider                                                  |
| --------- | --------------------------------------------------------- |
| STT       | `local-sherpa` (Sherpa-ONNX Zipformer on CPU)             |
| TTS       | `local-sherpa` (Sherpa-ONNX Piper/VITS offline synthesis) |

Your **browser microphone** → WebRTC → Node **VoiceAgent** → Sherpa `OnlineRecognizer` → partial/final events on the `voice-control` DataChannel.

**VAD / barge-in:** **energy VAD** via `VOICE_AGENT_VAD_PRESET` (`provider: 'energy'`, `threshold: 0.05`). Silero is optional and not in the shipped `.node` — see [`VOICE-VAD-AND-BARGE-IN.md`](../../packages/sdk/VOICE-VAD-AND-BARGE-IN.md#vad-providers-energy-vs-silero).

---

## Why use free local STT?

We **explicitly support and recommend** the `local-sherpa` flow when you can run inference on your own hardware:

| Benefit                       | What it means                                                                                                                |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| **Privacy**                   | User speech is decoded **on your server** — raw audio is **not** sent to OpenAI, Deepgram, Google, or other cloud STT APIs.  |
| **Lower latency**             | No network round-trip to a third-party STT service; transcripts come from in-process Sherpa inference after WebRTC delivery. |
| **No API keys or usage fees** | Download open-weight models once (`download-stt:*` scripts); run without STT cloud credentials.                              |
| **Offline-capable**           | After models are cached, STT works without outbound calls to speech vendors (WebRTC signaling still needs your network).     |

Cloud STT remains available via [`voice-agent-browser`](../voice-agent-browser/README.md) when you need vendor-specific features or languages without a local bundle. For agent loops that handle sensitive audio or care about tail latency, **start with this example**.

Pair `local-sherpa` for **both** STT and TTS for a fully on-device voice loop — user speech and agent replies stay off third-party cloud speech APIs.

---

## Architecture

```
Browser                         Node server
───────                         ───────────
getUserMedia ──WebRTC audio──►  VoiceAgent inbound (VAD + local Sherpa STT)
                                │
                                ▼
Event log ◄── DataChannel ◄── user_speech_partial / user_speech_final
   │
   └── { type: 'speak', text } ──► local Sherpa TTS ──► agent outbound track ──► browser <audio>
```

- **Signaling:** WebSocket on the same HTTP port as the static page (default **3002**)
- **Control channel:** label `voice-control` — see `wireVoiceAgentToDataChannel` in `@node-webrtc-rust/sdk/voice`
- **Model weights:** downloaded separately — **not** bundled in npm

### Multi-session / RAM (local Sherpa)

Each connected browser client gets its own `VoiceAgent` (see [`VoiceAgentSessionHost`](../../packages/helpers/src/voice-agent-session-host.ts)). Sherpa **model weights** are shared process-wide via `SherpaModelPool` (one `OnlineRecognizer` per `SHERPA_STT_MODEL_PATH`, one `OfflineTts` per TTS model dir); each session still has its own `OnlineStream` and VAD state.

Capacity planning, env limits (`SHERPA_POOL_MAX_CONCURRENT_DECODE`, `SHERPA_POOL_MAX_CONCURRENT_TTS`), and RAM/CPU tables:

`development/node-webrtc-rust/plans/2026-05-31-sherpa-shared-model-pool.md`

---

## Quick start

From the **repo root**, after `npm run setup`:

### 1. Download STT model weights (once)

```bash
npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

### 1b. Download TTS voice model (once)

```bash
npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Default English voice: **Piper Amy (low)** (`vits-piper-en_US-amy-low`). Other voices:

```bash
npm run download-tts:list --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:es --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:de --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

This fetches **English streaming Zipformer (Kroko)** STT (`sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06`, ~119 MB compressed) from the official Sherpa-ONNX releases and extracts it to:

```text
examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06/
```

Expected files inside:

```text
tokens.txt
encoder-epoch-99-avg-1.int8.onnx   (name may vary — must contain "encoder")
decoder-epoch-99-avg-1.onnx
joiner-epoch-99-avg-1.onnx
```

### Other languages (automatic download)

List all entries (including ones without a dedicated streaming bundle):

```bash
npm run download-stt:list --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Per-language shortcuts (from repo root) — full table also in [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md#local-sherpa-onnx--multilingual-models):

| Language             | npm script                          | Sherpa bundle                                                |
| -------------------- | ----------------------------------- | ------------------------------------------------------------ |
| English (default)    | `download-stt` or `download-stt:en` | `…-en-kroko-2025-08-06`                                      |
| English (2023 legacy) | `download-stt:en-legacy`           | `…-en-2023-06-26`                                            |
| Spanish              | `download-stt:es`                   | `…-es-kroko-2025-08-06`                                      |
| French               | `download-stt:fr`                   | `…-fr-kroko-2025-08-06`                                      |
| German               | `download-stt:de`                   | `…-de-kroko-2025-08-06`                                      |
| Chinese              | `download-stt:zh`                   | `…-zh-int8-2025-06-30`                                       |
| Japanese             | `download-stt:ja`                   | `…-ar_en_id_ja_ru_th_vi_zh-2025-02-10` (multilingual)        |
| Arabic               | `download-stt:ar`                   | same multilingual bundle — set `SHERPA_STT_LANGUAGE=ar`      |
| Russian              | `download-stt:ru`                   | `…-small-ru-vosk-int8-2025-08-16`                            |
| Bengali (South Asia) | `download-stt:bn`                   | `…-bn-vosk-2026-02-09`                                       |
| Hindi                | `download-stt:hi`                   | _No streaming Zipformer transducer in official releases yet_ |
| Portuguese           | `download-stt:pt`                   | _Not available for this example yet_                         |
| Italian              | `download-stt:it`                   | _Not available for this example yet_                         |

Generic form:

```bash
npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --lang=de
```

After download, export both path and language (the script prints them):

```bash
export SHERPA_STT_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06"
export SHERPA_STT_LANGUAGE=de
```

For the **multilingual** Japanese/Arabic bundle, always set `SHERPA_STT_LANGUAGE` to the language you are speaking (`ja`, `ar`, `ru`, `vi`, `id`, `th`, `zh`, or `en`).

### 2. Point the server at both model directories

From **`node-webrtc-rust`** repo root (paths printed by the download scripts):

```bash
export SHERPA_STT_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06"
export SHERPA_TTS_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low"
export SHERPA_TTS_SPEAKER=0   # optional — Piper speaker id for multi-speaker models
```

You can also set `stt.modelPath` in code instead of the env var (see [Configuration](#configuration)).

### 3. Start the server

```bash
npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

### 3b. Node-only TTS → STT roundtrip (no browser)

Verifies the full on-device loop without a microphone: **two VoiceAgents** (speaker TTS → WebRTC → listener STT + VAD + gateStt).

```bash
npm run build:native   # if you changed Rust since last build
npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run start:roundtrip --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- "I love America"
```

Default (no args) runs **5 built-in phrases** and checks **word similarity** (lowercased word overlap, default 75% threshold).

**Full documentation:** [`ROUNDTRIP.md`](./ROUNDTRIP.md) — architecture, VAD timing vs explicit silence, similarity, env vars, and defaults.

Quick reference:

| Variable                          | Default | Purpose                                                                                 |
| --------------------------------- | ------- | --------------------------------------------------------------------------------------- |
| `SHERPA_ROUNDTRIP_PHRASE`         | —       | Single phrase instead of the 5-sentence batch                                           |
| `SHERPA_ROUNDTRIP_MIN_SIMILARITY` | `0.75`  | Min fraction of input words matched in recognition                                      |
| `SHERPA_ROUNDTRIP_TIMEOUT_MS`     | `45000` | Per-phrase STT timeout                                                                  |
| `SHERPA_ROUNDTRIP_WARMUP_S`       | `0.6`   | Speaker warmup silence before first TTS (WebRTC priming)                                |
| `SHERPA_ROUNDTRIP_GAP_S`          | `0`     | **Extra** silence between phrases; off by default (VAD hold + trailing silence suffice) |
| `SHERPA_ROUNDTRIP_VERBOSE`        | off     | Log every VAD/STT event                                                                 |

Inter-phrase separation comes from **listener VAD** (`sttGateHoldMs`, endpoint tail, wait for `user_speech_final`) plus **post-TTS trailing silence** on the speaker track (duration derived from those VAD timings). See [ROUNDTRIP.md § Timing](./ROUNDTRIP.md#timing-vad-vs-explicit-silence).

### 3c. Barge-in E2E (interrupt TTS mid-playback)

Same loopback as §3b, but the **speaker** has VAD + `bargeIn`; the user leg injects tone on `agentInbound` after a delay. Phase 1 measures full TTS received on `userInbound`; phase 2 must be shorter and emit `barge_in`.

```bash
npm run start:roundtrip-barge-in --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Details: [ROUNDTRIP.md § Barge-in E2E](./ROUNDTRIP.md#barge-in-e2e).

Rust-level smoke (no WebRTC): `cargo test -p node-webrtc-rust-vendor-sherpa-onnx tts_synthesize_produces_stereo_pcm_with_model -- --ignored` with `SHERPA_TTS_MODEL_PATH` set.

**Debug pipeline (VAD, PCM, Sherpa, DataChannel):**

```bash
npm run build:native   # required — Rust logs live in the .node binary
npm run start:debug --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

On startup you should see `[voice-debug] JsVoiceAgent native module loaded`. If that line is **missing**, the native addon is stale — rerun `npm run build:native`.

Logs go to **stderr** with `[voice-debug]` and `[webrtc-debug]` prefixes. Debug mode also relaxes VAD (`threshold=0.01`, `gateStt=false`). Optional overrides:

| Variable                    | Effect                                |
| --------------------------- | ------------------------------------- |
| `VOICE_VAD_THRESHOLD=0.005` | Lower energy threshold                |
| `VOICE_VAD_DISABLED=1`      | Skip VAD (STT still receives all PCM) |

Startup logs show the active pipeline and model path:

```text
Local Sherpa voice demo at http://localhost:3002
Voice pipeline: local Sherpa-ONNX (browser mic → on-device STT)
STT=local-sherpa  TTS=local-sherpa
SHERPA_STT_MODEL_PATH=.../sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06
SHERPA_TTS_MODEL_PATH=.../vits-piper-en_US-amy-low
```

### 4. Open the browser client

Visit [http://localhost:3002](http://localhost:3002) (override with `PORT=`).

1. Allow **microphone** access.
2. Click **Connect**.
3. Speak — watch **`user_speech_partial`** and **`user_speech_final`** in the event log.
4. Optional: use **Speak** / **Speak long reply** to test on-device Piper TTS and **barge-in**.

---

## Manual model download (alternative)

If the download script fails (no `tar`, firewall, etc.):

1. Open [Sherpa-ONNX ASR model releases](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models)
2. Download `sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06.tar.bz2` (or `npm run download-stt:en`)
3. Extract anywhere on disk
4. Set `SHERPA_STT_MODEL_PATH` to the extracted folder containing `tokens.txt` and the three ONNX files

Other **streaming transducer** bundles work if they include `tokens.txt` + encoder/decoder/joiner `.onnx` files. See [`crates/vendor-sherpa-onnx/README.md`](../../crates/vendor-sherpa-onnx/README.md).

---

## Configuration

### Environment variables

| Variable                | Required | Purpose                                                                |
| ----------------------- | -------- | ---------------------------------------------------------------------- |
| `SHERPA_STT_MODEL_PATH` | **Yes**  | STT directory with Sherpa ONNX Zipformer weights                       |
| `SHERPA_TTS_MODEL_PATH` | **Yes**  | TTS directory with Piper/VITS `.onnx`, `tokens.txt`, `espeak-ng-data/` |
| `SHERPA_STT_LANGUAGE`   | No       | BCP-47-ish tag for `stt.language` (inferred from path when omitted)    |
| `SHERPA_TTS_SPEAKER`    | No       | Piper speaker id for `tts.voice` (default `0`)                         |
| `SHERPA_TTS_SPEED`      | No       | Speech speed multiplier passed via `tts.model` or env (default `1.0`)  |
| `PORT`                  | No       | HTTP + WebSocket port (default `3002`)                                 |

### VoiceAgent config (TypeScript)

The server resolves config in `src/resolve-voice-config.ts`:

```typescript
{
  stt: {
    provider: 'local-sherpa',
    language: 'en',
    modelPath: process.env.SHERPA_STT_MODEL_PATH,
  },
  tts: { provider: 'local-sherpa', modelPath: process.env.SHERPA_TTS_MODEL_PATH, voice: '0' },
  vad: { enabled: true, threshold: 0.05, bargeIn: { enabled: true, flushTts: true } },
}
```

To integrate in your own app:

```typescript
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

const agent = new VoiceAgent({
  stt: {
    provider: 'local-sherpa',
    language: 'en',
    modelPath: process.env.SHERPA_STT_MODEL_PATH,
  },
  tts: { provider: 'local-sherpa', modelPath: process.env.SHERPA_TTS_MODEL_PATH, voice: '0' },
  // tts: { provider: 'openai', apiKey: process.env.OPENAI_API_KEY }, // cloud fallback
})

await agent.attach({ inboundTrack, outboundTrack })
await agent.start()

agent.on('user_speech_partial', (e) => console.log('partial:', e.text))
agent.on('user_speech_final', (e) => console.log('final:', e.text))
```

Sherpa runs in Rust via `crates/vendor-sherpa-onnx` — included in the **default** native bindings build.

---

## How local STT works in this repo

1. WebRTC delivers **mono 16 kHz i16** PCM to `SttProvider::push_audio`
2. `SherpaStt` converts samples to **f32** and feeds `OnlineStream::accept_waveform`
3. Inference runs on a **blocking thread pool** (`spawn_blocking`) — never on the Tokio async worker
4. `poll_transcript` returns:
   - **`Partial`** when hypothesis text changes
   - **`Final`** when Sherpa endpoint detection fires, then the stream resets
5. Events flow through the VoiceAgent event bus → DataChannel bridge → browser log

---

## Troubleshooting

| Symptom                                           | Fix                                                                                                                                                                                                                              |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SHERPA_STT_MODEL_PATH is not set` on startup     | Run `download-stt` and export the path                                                                                                                                                                                           |
| `SHERPA_TTS_MODEL_PATH is not set` on startup     | Run `download-tts` and export the path                                                                                                                                                                                           |
| `no encoder .onnx model found`                    | Point `SHERPA_STT_MODEL_PATH` at the **extracted** folder, not the `.tar.bz2`                                                                                                                                                    |
| Empty partials / no finals                        | Run `start:debug` and check `[voice-debug]` — confirm inbound PCM bytes, Sherpa hypotheses, and `voice-control send` lines; try `VOICE_VAD_DISABLED=1`                                                                           |
| Native load error / missing symbol                | Rebuild: `npm run build:native` from repo root                                                                                                                                                                                   |
| **`npm` exit code 137** (process killed silently) | macOS stale code signature on `.node` after Sherpa rebuild — from repo root: `npm run build:native`, or `codesign --force --sign - packages/bindings/node-webrtc-rust.node packages/bindings/node-webrtc-rust.darwin-arm64.node` |
| Slow on older CPU                                 | Use a smaller/int8 model; expect higher latency                                                                                                                                                                                  |
| Port in use                                       | `PORT=3003 npm run start --workspace=...`                                                                                                                                                                                        |

---

## Data channel protocol

Same as [`voice-agent-browser`](../voice-agent-browser/README.md):

| Direction       | Payload                                                                 |
| --------------- | ----------------------------------------------------------------------- |
| Client → server | `{ "type": "speak", "text": "Hello" }`                                  |
| Server → client | `{ "type": "speech_event", "event": "user_speech_final", "text": "…" }` |

---

## Related

- Cloud vendor browser demo: [`voice-agent-browser`](../voice-agent-browser/README.md)
- **Official STT/TTS API reference:** [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md) (multilingual Sherpa table)
- Rust adapter: [`crates/vendor-sherpa-onnx`](../../crates/vendor-sherpa-onnx/README.md)
- SDK bridge: [`packages/sdk/src/voice/speech-event-bridge.ts`](../../packages/sdk/src/voice/speech-event-bridge.ts)
- Plan / design notes: `development/node-webrtc-rust/plans/2026-05-28-vendor-sherpa-onnx.md`
