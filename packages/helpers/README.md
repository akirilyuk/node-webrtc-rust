# @node-webrtc-rust/helpers

Reusable **server-side** TypeScript helpers for voice agent apps — the same abstractions used in the examples, published as a package so you can skip copy-pasting negotiation boilerplate.

## Install

```bash
npm install @node-webrtc-rust/helpers @node-webrtc-rust/sdk @node-webrtc-rust/signaling
```

## What's included

| Export | Purpose |
| --- | --- |
| `SessionPod` | One Node process, one signaling entry point, many concurrent sessions |
| `VoiceAgentSessionHost` | One signaling room; spawns one `VoiceAgent` + PC per browser client |
| `VOICE_AGENT_SERVER_PEER_ID` | Stable server peer id for signaling joins |
| `createKickFrame`, PCM constants | RTP prime / 20 ms frame conventions |

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

## PCM kick frame

```typescript
import { createKickFrame, PCM_KICK_DURATION_MS } from '@node-webrtc-rust/helpers/pcm'

await localTrack.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
```

## Design notes

- **One `VoiceAgent` per WebRTC connection** — intentional; no multiplexing inside the agent.
- **Disconnect = cleanup** — `VoiceAgent.stop()` and `RTCPeerConnection.close()` run in `VoiceAgentSessionHost`.
- **Scale pods by CPU/RAM**, not one process per call — see multi-session pod example README.
