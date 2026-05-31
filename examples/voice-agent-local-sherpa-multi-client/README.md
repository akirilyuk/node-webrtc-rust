# Local Sherpa — three clients, one room

Demonstrates the **reusable multi-client server pattern** for local Sherpa STT/TTS:

- **One Node process**, **one signaling room** (`sherpa-multi` by default)
- **Three browser tabs** → three `client-tab*` peers → **three `VoiceAgent` instances**
- **One shared Sherpa model load** in Rust (`SherpaModelPool`) — not three copies of ONNX weights
- Optional **`VOICE_MAX_CONCURRENT_SESSIONS`** to reject extra tabs (deployment sizing)

## Architecture

```text
Tab 1 (client-tab1) ──WebRTC──┐
Tab 2 (client-tab2) ──WebRTC──┼── VoiceAgentSessionHost (one room)
Tab 3 (client-tab3) ──WebRTC──┘         │
                                        ├── VoiceAgent #1 + OnlineStream
                                        ├── VoiceAgent #2 + OnlineStream
                                        └── VoiceAgent #3 + OnlineStream
                                                    │
                                    shared OnlineRecognizer + OfflineTts (Rust pool)
```

For **many independent calls** (different room ids per customer), use [`voice-agent-multi-session-pod`](../voice-agent-multi-session-pod/) and `SessionPod` instead.

## Reusable library API

| Export                                                                                   | Use when                            |
| ---------------------------------------------------------------------------------------- | ----------------------------------- |
| [`startMultiClientVoiceServer`](../../packages/helpers/src/multi-client-voice-server.ts) | One room, N browser clients         |
| [`VoiceSessionBudget`](../../packages/helpers/src/voice-session-budget.ts)               | Cap connections per process         |
| [`SessionPod`](../../packages/helpers/src/session-pod.ts)                                | Many rooms / session ids in one pod |

This example’s server is ~30 lines: resolve Sherpa config → `startMultiClientVoiceServer({ serveHttp })`.

## Prerequisites

Same model downloads as [`voice-agent-local-sherpa`](../voice-agent-local-sherpa/README.md):

```bash
npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
export SHERPA_STT_MODEL_PATH=.../voice-agent-local-sherpa/.models/<stt-bundle>
export SHERPA_TTS_MODEL_PATH=.../voice-agent-local-sherpa/.models/<tts-bundle>
```

## Run

```bash
npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
```

Open **three tabs** to [http://localhost:3004](http://localhost:3004):

- `http://localhost:3004/?slot=1`
- `http://localhost:3004/?slot=2`
- `http://localhost:3004/?slot=3`

Click **Connect** in each (allow microphone). Speak or use **Speak** — each tab has its own agent and event log.

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

```bash
npm run test --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
npm run test --workspace=@node-webrtc-rust/helpers
```

## Environment

| Variable                        | Default         | Purpose                     |
| ------------------------------- | --------------- | --------------------------- |
| `PORT`                          | `3004`          | HTTP + WebSocket            |
| `VOICE_ROOM`                    | `sherpa-multi`  | Signaling room              |
| `VOICE_MAX_CONCURRENT_SESSIONS` | `0` (unlimited) | Process-wide connection cap |
| `SHERPA_STT_MODEL_PATH`         | —               | Required                    |
| `SHERPA_TTS_MODEL_PATH`         | —               | Required                    |

See also [`development/node-webrtc-rust/plans/2026-05-31-voice-session-budget.md`](../../development/node-webrtc-rust/plans/2026-05-31-voice-session-budget.md) and [`2026-05-31-sherpa-shared-model-pool.md`](../../development/node-webrtc-rust/plans/2026-05-31-sherpa-shared-model-pool.md).
