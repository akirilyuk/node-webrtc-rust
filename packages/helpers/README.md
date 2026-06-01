# @node-webrtc-rust/helpers

Reusable **server-side** TypeScript helpers for voice agent apps — the same abstractions used in the examples, published as a package so you can skip copy-pasting negotiation boilerplate.

## Install

```bash
npm install @node-webrtc-rust/helpers @node-webrtc-rust/sdk @node-webrtc-rust/signaling

## Tests

From the **repo root** (not inside `packages/helpers/`):

```bash
npm run test:helpers
```

CI runs the same via [`scripts/ci/run-helpers-unit-tests.sh`](../../scripts/ci/run-helpers-unit-tests.sh) in the **Typecheck & lint** job (`build.yml` on PRs, `build-main.yml` on push to `main`). Before push (lint + helpers vitest when those paths changed):

```bash
npm run ci:pre-push
```
```

## What's included

| Export                                 | Purpose                                                               |
| -------------------------------------- | --------------------------------------------------------------------- |
| `SessionPod`                           | One Node process, one signaling entry point, many concurrent sessions |
| `VoiceAgentSessionHost`                | One signaling room; spawns one `VoiceAgent` + PC per browser client   |
| `startMultiClientVoiceServer`          | One room, many tabs — wraps signaling + host + `/api/capacity`        |
| `VoiceAgentSessionHost.broadcastSpeak` | TTS `text` on every connected client in the room                      |
| `VoiceSessionHandler`                  | Per-tab hooks: `onSpeechEvent` (STT/VAD) and `onSpeakRequest` (TTS)   |
| `VoiceSessionBudget`                   | Process-wide cap (`VOICE_MAX_CONCURRENT_SESSIONS`)                    |
| `VOICE_AGENT_SERVER_PEER_ID`           | Stable server peer id for signaling joins                             |
| `createKickFrame`, PCM constants       | RTP prime / 20 ms frame conventions                                   |

## Multi-session pod (recommended server pattern)

```typescript
import { createServer } from 'http'

import { SignalingServer } from '@node-webrtc-rust/signaling'
import { SessionPod } from '@node-webrtc-rust/helpers'

const PORT = 3003
const httpServer = createServer()
const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
await signaling.listen(PORT)

const pod = new SessionPod(signaling, {
  signalingUrl: `ws://127.0.0.1:${PORT}/ws`,
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
  voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } },
})

// Orchestrator assigns a session id per call
await pod.ensureSession('call-abc123')

// Browser client joins signaling room `call-abc123` as `client-*`
// → pod creates RTCPeerConnection + VoiceAgent
// → on hangup: agent.stop(), PC close, optional idle session teardown
```

Runnable demo: [`examples/voice-agent-multi-session-pod`](../../examples/voice-agent-multi-session-pod/).

## Single room, many clients

If you only need one signaling room (not full pod orchestration):

```typescript
import { SignalingClient } from '@node-webrtc-rust/signaling'
import { VOICE_AGENT_SERVER_PEER_ID, VoiceAgentSessionHost } from '@node-webrtc-rust/helpers'

const signaling = new SignalingClient({
  url: 'ws://127.0.0.1:3001/ws',
  room: 'demo',
  peerId: VOICE_AGENT_SERVER_PEER_ID,
})
await signaling.connect()

const host = new VoiceAgentSessionHost(signaling, iceServers, { voiceConfig })
// Each `client-*` peer → own VoiceAgent; disconnect cleans up automatically
```

See [`examples/voice-agent-browser`](../../examples/voice-agent-browser/).

## Session budget (deployment sizing)

```bash
export VOICE_MAX_CONCURRENT_SESSIONS=8
```

`VoiceAgentSessionHost` and `SessionPod` share one process-wide budget. When full, new `client-*` peers are rejected (no WebRTC offer). Expose metrics via `GET /api/capacity` when using `startMultiClientVoiceServer`.

Demo: [`examples/voice-agent-local-sherpa-multi-client`](../../examples/voice-agent-local-sherpa-multi-client/) (three tabs, one room, local Sherpa).

## Multi-client room (three agents, shared Sherpa)

```typescript
import { startMultiClientVoiceServer } from '@node-webrtc-rust/helpers'

const server = await startMultiClientVoiceServer({
  port: 3004,
  room: 'sherpa-multi',
  voiceConfig: {
    stt: { provider: 'local-sherpa', modelPath },
    tts: { provider: 'local-sherpa', modelPath },
  },
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
  serveHttp: myStaticHandler,
})
// server.budget → { active, max, available, rejectedTotal }
```

Each browser tab = one `VoiceAgent`; Sherpa ONNX weights are pooled in the native layer.

### Custom STT / TTS logic

Pass a `VoiceSessionHandler` (or set `hostOptions.voiceHandler` on the host). Use `ctx.speak(text)` to play TTS on that tab's outbound track.

```typescript
import { startMultiClientVoiceServer, type VoiceSessionHandler } from '@node-webrtc-rust/helpers'

const voiceHandler: VoiceSessionHandler = {
  async onSpeechEvent(ctx, event) {
    if (event.type === 'user_speech_final' && event.text) {
      await ctx.speak(`You said: ${event.text}`)
    }
  },
  async onSpeakRequest(ctx, text) {
    await ctx.speak(text)
  },
}

await startMultiClientVoiceServer({
  port: 3004,
  room: 'demo',
  voiceConfig,
  iceServers,
  voiceHandler,
})
```

Runnable template: edit **`examples/voice-agent-local-sherpa-multi-client/src/voice-handler.ts`** only — see that example's README.

## PCM kick frame

```typescript
import { createKickFrame, PCM_KICK_DURATION_MS } from '@node-webrtc-rust/helpers/pcm'

await localTrack.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
```

## Design notes

- **One `VoiceAgent` per WebRTC connection** — intentional; no multiplexing inside the agent.
- **Disconnect = cleanup** — `VoiceAgent.stop()` and `RTCPeerConnection.close()` run in `VoiceAgentSessionHost`.
- **Scale pods by CPU/RAM**, not one process per call — see multi-session pod example README.
