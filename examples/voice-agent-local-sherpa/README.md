# Local Sherpa STT — Browser + Node

**On-device streaming speech-to-text** with [Sherpa-ONNX](https://github.com/k2-fsa/sherpa-onnx) — no cloud API keys.

This example mirrors [`voice-agent-browser`](../voice-agent-browser/README.md) but uses:

| Component | Provider |
|-----------|----------|
| STT | `local-sherpa` (Sherpa-ONNX Zipformer on CPU) |
| TTS | `mock` (deterministic playback for barge-in demos) |

Your **browser microphone** → WebRTC → Node **VoiceAgent** → Sherpa `OnlineRecognizer` → partial/final events on the `voice-control` DataChannel.

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
   └── { type: 'speak', text } ──► mock TTS ──► agent outbound track ──► browser <audio>
```

- **Signaling:** WebSocket on the same HTTP port as the static page (default **3002**)
- **Control channel:** label `voice-control` — see `wireVoiceAgentToDataChannel` in `@node-webrtc-rust/sdk/voice`
- **Model weights:** downloaded separately — **not** bundled in npm

---

## Quick start

From the **repo root**, after `npm run setup`:

### 1. Download model weights (once)

```bash
npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

This fetches **English streaming Zipformer** (`sherpa-onnx-streaming-zipformer-en-2023-06-26`, ~70 MB compressed) from the official Sherpa-ONNX releases and extracts it to:

```text
examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-2023-06-26/
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
npm run download-model:list --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

Per-language shortcuts (from repo root) — full table also in [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md#local-sherpa-onnx--multilingual-models):

| Language | npm script | Sherpa bundle |
|----------|------------|---------------|
| English (default) | `download-model` or `download-model:en` | `…-en-2023-06-26` |
| Spanish | `download-model:es` | `…-es-kroko-2025-08-06` |
| French | `download-model:fr` | `…-fr-kroko-2025-08-06` |
| German | `download-model:de` | `…-de-kroko-2025-08-06` |
| Chinese | `download-model:zh` | `…-zh-int8-2025-06-30` |
| Japanese | `download-model:ja` | `…-ar_en_id_ja_ru_th_vi_zh-2025-02-10` (multilingual) |
| Arabic | `download-model:ar` | same multilingual bundle — set `SHERPA_LANGUAGE=ar` |
| Russian | `download-model:ru` | `…-small-ru-vosk-int8-2025-08-16` |
| Bengali (South Asia) | `download-model:bn` | `…-bn-vosk-2026-02-09` |
| Hindi | `download-model:hi` | *No streaming Zipformer transducer in official releases yet* |
| Portuguese | `download-model:pt` | *Not available for this example yet* |
| Italian | `download-model:it` | *Not available for this example yet* |

Generic form:

```bash
npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --lang=de
```

After download, export both path and language (the script prints them):

```bash
export SHERPA_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06"
export SHERPA_LANGUAGE=de
```

For the **multilingual** Japanese/Arabic bundle, always set `SHERPA_LANGUAGE` to the language you are speaking (`ja`, `ar`, `ru`, `vi`, `id`, `th`, `zh`, or `en`).

### 2. Point the server at the model directory

From **`node-webrtc-rust`** repo root:

```bash
export SHERPA_MODEL_PATH="$PWD/examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-2023-06-26"
```

You can also set `stt.modelPath` in code instead of the env var (see [Configuration](#configuration)).

### 3. Start the server

```bash
npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

**Debug pipeline (VAD, PCM, Sherpa, DataChannel):**

```bash
npm run build:native   # required — Rust logs live in the .node binary
npm run start:debug --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

On startup you should see `[voice-debug] JsVoiceAgent native module loaded`. If that line is **missing**, the native addon is stale — rerun `npm run build:native`.

Logs go to **stderr** with `[voice-debug]` and `[webrtc-debug]` prefixes. Debug mode also relaxes VAD (`threshold=0.01`, `gateStt=false`). Optional overrides:

| Variable | Effect |
|----------|--------|
| `VOICE_VAD_THRESHOLD=0.005` | Lower energy threshold |
| `VOICE_VAD_DISABLED=1` | Skip VAD (STT still receives all PCM) |

Startup logs show the active pipeline and model path:

```text
Local Sherpa voice demo at http://localhost:3002
Voice pipeline: local Sherpa-ONNX (browser mic → on-device STT)
STT=local-sherpa  TTS=mock
SHERPA_MODEL_PATH=.../sherpa-onnx-streaming-zipformer-en-2023-06-26
```

### 4. Open the browser client

Visit [http://localhost:3002](http://localhost:3002) (override with `PORT=`).

1. Allow **microphone** access.
2. Click **Connect**.
3. Speak — watch **`user_speech_partial`** and **`user_speech_final`** in the event log.
4. Optional: use **Speak** / **Speak long reply** to test mock TTS and **barge-in**.

---

## Manual model download (alternative)

If the download script fails (no `tar`, firewall, etc.):

1. Open [Sherpa-ONNX ASR model releases](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models)
2. Download `sherpa-onnx-streaming-zipformer-en-2023-06-26.tar.bz2`
3. Extract anywhere on disk
4. Set `SHERPA_MODEL_PATH` to the extracted folder containing `tokens.txt` and the three ONNX files

Other **streaming transducer** bundles work if they include `tokens.txt` + encoder/decoder/joiner `.onnx` files. See [`crates/vendor-sherpa-onnx/README.md`](../../crates/vendor-sherpa-onnx/README.md).

---

## Configuration

### Environment variables

| Variable | Required | Purpose |
|----------|----------|---------|
| `SHERPA_MODEL_PATH` | **Yes** | Directory with Sherpa ONNX weights |
| `SHERPA_LANGUAGE` | No | BCP-47-ish tag for `stt.language` (inferred from path when omitted) |
| `PORT` | No | HTTP + WebSocket port (default `3002`) |

### VoiceAgent config (TypeScript)

The server resolves config in `src/resolve-voice-config.ts`:

```typescript
{
  stt: {
    provider: 'local-sherpa',
    language: 'en',
    modelPath: process.env.SHERPA_MODEL_PATH,
  },
  tts: { provider: 'mock', voice: 'demo' },
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
    modelPath: process.env.SHERPA_MODEL_PATH,
  },
  tts: { provider: 'openai', apiKey: process.env.OPENAI_API_KEY }, // or mock / other vendor
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

| Symptom | Fix |
|---------|-----|
| `SHERPA_MODEL_PATH is not set` on startup | Run `download-model` and export the path |
| `no encoder .onnx model found` | Point `SHERPA_MODEL_PATH` at the **extracted** folder, not the `.tar.bz2` |
| Empty partials / no finals | Run `start:debug` and check `[voice-debug]` — confirm inbound PCM bytes, Sherpa hypotheses, and `voice-control send` lines; try `VOICE_VAD_DISABLED=1` |
| Native load error / missing symbol | Rebuild: `npm run build:native` from repo root |
| **`npm` exit code 137** (process killed silently) | macOS stale code signature on `.node` after Sherpa rebuild — from repo root: `npm run build:native`, or `codesign --force --sign - packages/bindings/node-webrtc-rust.node packages/bindings/node-webrtc-rust.darwin-arm64.node` |
| Slow on older CPU | Use a smaller/int8 model; expect higher latency |
| Port in use | `PORT=3003 npm run start --workspace=...` |

---

## Data channel protocol

Same as [`voice-agent-browser`](../voice-agent-browser/README.md):

| Direction | Payload |
|-----------|---------|
| Client → server | `{ "type": "speak", "text": "Hello" }` |
| Server → client | `{ "type": "speech_event", "event": "user_speech_final", "text": "…" }` |

---

## Related

- Cloud vendor browser demo: [`voice-agent-browser`](../voice-agent-browser/README.md)
- **Official STT/TTS API reference:** [`examples/shared/VOICE_VENDOR_REFERENCE.md`](../shared/VOICE_VENDOR_REFERENCE.md) (multilingual Sherpa table)
- Rust adapter: [`crates/vendor-sherpa-onnx`](../../crates/vendor-sherpa-onnx/README.md)
- SDK bridge: [`packages/sdk/src/voice/speech-event-bridge.ts`](../../packages/sdk/src/voice/speech-event-bridge.ts)
- Plan / design notes: `development/node-webrtc-rust/plans/2026-05-28-vendor-sherpa-onnx.md`
