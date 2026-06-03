# Local Sherpa — three clients, one room

Demonstrates the **reusable multi-client server pattern** for local Sherpa STT/TTS:

- **One Node process**, **one signaling room** (`sherpa-multi` by default)
- **Three browser tabs** → three `client-tab*` peers → **three `VoiceAgent` instances**
- **One shared Sherpa model load** in Rust (`SherpaModelPool`) — not three copies of ONNX weights
- Optional **`VOICE_MAX_CONCURRENT_SESSIONS`** to reject extra tabs (deployment sizing)

## Quick start

From **`node-webrtc-rust`** repo root.

**1. One-time — download English models** (skip if already done):

```bash
npm run download-stt:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

**2. Start the server** (model paths are set automatically — no manual `export SHERPA_*`):

```bash
npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
```

Before `tsx` runs, npm **`prestart`** calls [`scripts/free-port.sh`](../../scripts/free-port.sh) to stop anything still listening on port **3004** (or your `PORT`). The server also calls [`shared/free-port.ts`](../shared/free-port.ts) at startup as a backup.

**3. Open three browser tabs** to [http://127.0.0.1:3004](http://127.0.0.1:3004) (or use `?slot=1`, `?slot=2`, `?slot=3`). Click **Connect** in each tab, allow the microphone, then speak — each tab gets its own TTS reply on `user_speech_final`.

Optional:

```bash
# Third tab rejected when two sessions are active
npm run start:cap-2 --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client

# Verbose voice / WebRTC logs
npm run start:debug --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
```

## npm scripts

| Script          | What it does |
| --------------- | ------------ |
| `prestart`      | `free-port.sh` on `PORT` (default **3004**) |
| `start`         | Export Sherpa model paths → `tsx src/index.ts` |
| `start:cap-2`   | Same with `VOICE_MAX_CONCURRENT_SESSIONS=2` |
| `start:debug`   | Same with `VOICE_DEBUG=1` and `WEBRTC_DEBUG=1` |
| `test`          | Vitest (session budget; no models) |
| `typecheck`     | `tsc --noEmit` |

Model env vars (`SHERPA_STT_MODEL_PATH`, `SHERPA_TTS_MODEL_PATH`, `SHERPA_STT_LANGUAGE`) are exported by [`scripts/export-sherpa-local-models.sh`](../../scripts/export-sherpa-local-models.sh) on every `start*`. To set them in your shell instead:

```bash
source scripts/export-sherpa-local-models.sh
```

## Your code

Edit **`src/voice-handler.ts`** only:

| Handler            | When it runs                                      | TTS scope                                  |
| ------------------ | ------------------------------------------------- | ------------------------------------------ |
| `onSpeechEvent`    | VAD, STT partial/final, TTS lifecycle, barge-in   | **This tab only** — `ctx.speak(reply)`     |
| `onSpeakRequest`   | Browser Speak form (`{ type: 'speak' }`)          | **This tab only**                          |
| `onBroadcastSpeak` | Page “Speak to all” / `POST /api/broadcast-speak` | **Every connected tab** — not used for STT |

The example echoes finals: _"You said: …"_ via TTS. Events are still mirrored to each tab’s browser event log.

`src/index.ts` only boots HTTP/signaling and passes `voiceHandler` into `startMultiClientVoiceServer`.

## Architecture

```text
Tab 1 (client-tab1) ──WebRTC──┐
Tab 2 (client-tab2) ──WebRTC──┼── VoiceAgentSessionHost (one room)
Tab 3 (client-tab3) ──WebRTC──┘         │
                                        ├── VoiceAgent #1 → voice-handler.ts
                                        ├── VoiceAgent #2 → voice-handler.ts
                                        └── VoiceAgent #3 → voice-handler.ts
                                                    │
                                    shared OnlineRecognizer + OfflineTts (Rust pool)
```

For **many independent calls** (different room ids per customer), use [`voice-agent-multi-session-pod`](../voice-agent-multi-session-pod/) and `SessionPod` instead.

## Multiple tabs and microphone capture

This demo is **not** “one mic for the whole browser.” Each tab that clicks **Connect** calls `getUserMedia` and sends its own WebRTC audio stream to the server for as long as that tab stays **open and connected**.

| Question                                              | Answer                                                                                                                                    |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Must the tab be **focused** to send audio?            | **No.** By default, background tabs keep sending (normal WebRTC behavior).                                                                |
| Does a **closed** or **Disconnected** tab send audio? | **No** — tracks are stopped.                                                                                                              |
| Why can all three tabs “hear” me when I speak once?   | One physical mic; three agents run STT. Room sound (or speakers) can trigger **multiple** `user_speech_final` events — not broadcast TTS. |

### Pause mic in background (per tab)

Each tab has a checkbox under **Connect** → **Multi-tab microphone**: **“Pause mic when this tab is in the background.”** Or open with `?pauseMicBackground=1` (handy for `?slot=2` / `?slot=3`).

- **Default: off** — matches production-style always-on capture while connected.
- **On** — sets `MediaStreamTrack.enabled = false` when `document.hidden`, so that tab stops sending audio until you focus it again. Use this when testing at one desk to avoid background tabs picking up speech.

Production apps often use one client per user, push-to-talk, or visibility-based mute; this control documents the tradeoff for the three-tab demo.

## Reusable library API

| Export                                                                                   | Use when                            |
| ---------------------------------------------------------------------------------------- | ----------------------------------- |
| [`startMultiClientVoiceServer`](../../packages/helpers/src/multi-client-voice-server.ts) | One room, N browser clients         |
| [`VoiceSessionHandler`](../../packages/helpers/src/voice-session-handler.ts)             | Per-tab STT/TTS app logic           |
| [`VoiceSessionBudget`](../../packages/helpers/src/voice-session-budget.ts)               | Cap connections per process         |
| [`SessionPod`](../../packages/helpers/src/session-pod.ts)                                | Many rooms / session ids in one pod |

## Prerequisites

Same model downloads as [`voice-agent-local-sherpa`](../voice-agent-local-sherpa/README.md). Only the download step is required — `npm run start` wires model paths via [`scripts/export-sherpa-local-models.sh`](../../scripts/export-sherpa-local-models.sh):

```bash
npm run download-stt:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
```

For a non-English STT bundle, download with the matching `download-stt:*` script, then override before start:

```bash
export SHERPA_STT_BUNDLE=sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06
export SHERPA_STT_LANGUAGE=de
npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
```

## Troubleshooting connect

| Symptom | What to check |
| ------- | ------------- |
| Event log stops at `joined as client-tab…` | Server terminal must show `[voice client-tab…] offer sent`. If not: restart `npm run start`, click **Disconnect** then **Connect**, or use a fresh `?slot=1` tab so the peer id is new. |
| Stops after `received WebRTC offer`, no `answer sent` for ~15–40 s then **failed** | Old clients blocked the answer on full ICE gathering. Current client sends **answer immediately** (trickle ICE). Pull latest, hard-refresh the tab. |
| `iceConnectionState=failed` then reconnect works | Use **`http://127.0.0.1:3004`** and `WEBRTC_NAT_1TO1_IPS=127.0.0.1` on the server (`npm run build:native` after core changes). Server auto-sends up to **2** new offers after `connectionState=failed`. |
| `iceConnectionState=failed` / WebRTC **failed** | Server SDP must include **`127.0.0.1` ICE host candidates**. `npm run start` sets `WEBRTC_NAT_1TO1_IPS=127.0.0.1`. |
| Server: `timed out waiting for mic track` | Usually ICE never connected (no answer or no `127.0.0.1` in offer). Fix ICE first; server no longer crashes the process on this timeout. |
| `no offer within 15s` | Server not running, wrong port, or `VOICE_MAX_CONCURRENT_SESSIONS` full (`start:cap-2` + three tabs). |
| Server exit code **137** | Previous listener was killed (`prestart` / `free-port.sh`) or OOM — start the server again and reconnect browsers. |
| `ignored duplicate offer` | Click **Disconnect** before **Connect** again, or wait for server reconnect after ICE failed (replaces dead `RTCPeerConnection`). |
| Event log shows `srv=…` on speech lines | Server timestamp when the event was forwarded on `voice-control` (compare to local time on the left). |

After a code fix in `@node-webrtc-rust/helpers`, rebuild if needed: `npm run build --workspace=@node-webrtc-rust/helpers`.

## Port cleanup (`EADDRINUSE`)

| Layer | When | How |
| ----- | ---- | --- |
| **Shell** | npm `prestart` (before Sherpa export / `tsx`) | [`scripts/free-port.sh`](../../scripts/free-port.sh) |
| **TypeScript** | `main()` in `src/index.ts` | [`shared/free-port.ts`](../shared/free-port.ts) |

Both use the same port as the server (`PORT`, default **3004**). Disable both with `VOICE_SKIP_FREE_PORT=1`.

From repo root (manual):

```bash
PORT=3004 bash scripts/free-port.sh voice-agent-local-sherpa-multi-client
# or custom port:
PORT=3010 bash scripts/free-port.sh voice-agent-local-sherpa-multi-client
```

See [`scripts/README.md`](../../scripts/README.md#free-portsh).

## Run (browser)

Open **three tabs** to [http://localhost:3004](http://localhost:3004):

- `http://localhost:3004/?slot=1`
- `http://localhost:3004/?slot=2`
- `http://localhost:3004/?slot=3`

Click **Connect** in each (allow microphone). Speak — on `user_speech_final` the server runs your handler and plays TTS. Use **Speak** to test `onSpeakRequest`.

### Broadcast TTS to every connected tab

Any tab can use **Speak to all** on the page (or call the API):

```bash
curl -s -X POST http://localhost:3004/api/broadcast-speak \
  -H 'Content-Type: application/json' \
  -d '{"text":"Hello everyone"}' | jq .
```

The server runs TTS on each active `client-*` peer (`VoiceAgentSessionHost.broadcastSpeak`).

### Test session cap

```bash
npm run start:cap-2 --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
```

Connect tabs 1 and 2 — OK. Tab 3 should stay without an offer; server log shows `rejected — session budget full`.

Check capacity:

```bash
curl -s http://localhost:3004/api/capacity | jq .
```

## Tests (no Sherpa models)

From repo root (CI runs this in the quality job):

```bash
npm run test:helpers
```

## Environment

| Variable                        | Default         | Purpose                               |
| ------------------------------- | --------------- | ------------------------------------- |
| `PORT`                          | `3004`          | HTTP + WebSocket; `prestart` frees this port |
| `VOICE_SKIP_FREE_PORT`          | unset           | Set to `1` to skip shell + TS port cleanup |
| `VOICE_ROOM`                    | `sherpa-multi`  | Signaling room                        |
| `VOICE_MAX_CONCURRENT_SESSIONS` | `0` (unlimited) | Process-wide connection cap           |
| `SHERPA_STT_MODEL_PATH`         | auto (English Kroko) | Set by `export-sherpa-local-models.sh` on `npm run start*` |
| `SHERPA_TTS_MODEL_PATH`         | auto (Piper amy-low) | Set by `export-sherpa-local-models.sh` on `npm run start*` |
| `SHERPA_STT_BUNDLE`             | English Kroko bundle | Override STT directory name under `.models/`               |
| `SHERPA_TTS_BUNDLE`             | `vits-piper-en_US-amy-low` | Override TTS directory name under `.models/`          |
| `VOICE_VAD_MIN_SILENCE_MS`      | preset `1300`   | Silence before “maybe done” / STT gate hold starts |
| `VOICE_VAD_STT_GATE_HOLD_MS`    | preset `1000`   | After maybe-done, STT stays open ~1 s for word gaps; `user_speaking_end` when hold expires |

VAD/STT flow tuning: [`packages/sdk/VOICE-VAD-AND-BARGE-IN.md`](../../packages/sdk/VOICE-VAD-AND-BARGE-IN.md#stt-flow-fine-tuning-gatestt).

See also [`development/node-webrtc-rust/plans/2026-05-31-voice-session-budget.md`](../../development/node-webrtc-rust/plans/2026-05-31-voice-session-budget.md) and [`2026-05-31-sherpa-shared-model-pool.md`](../../development/node-webrtc-rust/plans/2026-05-31-sherpa-shared-model-pool.md).
