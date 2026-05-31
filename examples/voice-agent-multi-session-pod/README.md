# Multi-Session Voice Pod

Demonstrates the **recommended server scaling model**: one Node pod, one signaling entry point, many concurrent voice sessions — with **one `VoiceAgent` per WebRTC connection** and automatic cleanup when the call ends.

Implementation lives in [`@node-webrtc-rust/helpers`](../../packages/helpers/README.md) (`SessionPod`, `VoiceAgentSessionHost`). This example is a thin HTTP demo around those helpers.

## Why this pattern

| Idea | Detail |
| --- | --- |
| **One pod, many sessions** | A single process runs one `SignalingServer` and handles N independent calls. |
| **One agent per connection** | Each WebRTC client gets its own `RTCPeerConnection` + `VoiceAgent`. No routing logic inside the agent. |
| **Disconnect = cleanup** | `VoiceAgent.stop()`, PC close, tracks released — no leaked STT/TTS/VAD state. |
| **Idle session teardown** | When the last client leaves a session room, the pod drops that session slot. |

Scale horizontally by running more pods when CPU/memory warrants it — not because the library requires one process per call.

**Local Sherpa (`local-sherpa`):** STT/TTS ONNX weights are **pooled per model path** in the native layer (one recognizer + one TTS engine per bundle per process). Session count still drives CPU (decode/TTS work) and per-session stream state — see `development/node-webrtc-rust/plans/2026-05-31-sherpa-shared-model-pool.md`.

## Architecture

```
Browser tab (session call-1) ──WebRTC──► VoiceAgent #1 ──┐
Browser tab (session call-2) ──WebRTC──► VoiceAgent #2 ──┼── SessionPod (one Node process)
Browser tab (session call-3) ──WebRTC──► VoiceAgent #3 ──┘
                              ▲
                              └── SignalingServer ws://host:3003/ws
```

## Run

```bash
npm run start --workspace=@node-webrtc-rust/example-voice-agent-multi-session-pod
```

Open [http://localhost:3003](http://localhost:3003) (override with `PORT=`).

1. Tab A: session `call-1` → Connect → speak
2. Tab B: session `call-2` → Connect → speak (same pod, independent agents)
3. Disconnect tab A → server log shows agent stop + session teardown

Mock STT/TTS by default (no API keys). Same `VOICE_VENDOR` env vars as [`voice-agent-browser`](../voice-agent-browser/README.md) if you want live vendors.

## Pod API (demo HTTP)

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/api/sessions` | `{ "sessionId": "call-1" }` — allocate session on pod |
| `GET` | `/api/sessions` | `{ activeSessions, activeConnections, sessions[] }` |

In production, call `SessionPod.ensureSession()` from your orchestrator instead of this demo route.

## Use in your app

```typescript
import { SessionPod } from '@node-webrtc-rust/helpers'

const pod = new SessionPod(signaling, { signalingUrl, iceServers, voiceConfig })
await pod.ensureSession(sessionId)
// Client joins ws://pod/ws room=sessionId → one VoiceAgent per connection
```

Full API docs: [`packages/helpers/README.md`](../../packages/helpers/README.md).

## Related examples

| Example | Focus |
| --- | --- |
| [`voice-agent-browser`](../voice-agent-browser/) | Single-room browser demo using `VoiceAgentSessionHost` |
| [`voice-agent`](../voice-agent/) | CLI loopback — one agent, one loop |
| [`conference-room`](../conference-room/) | Multi-party MCU mixing — different problem |
