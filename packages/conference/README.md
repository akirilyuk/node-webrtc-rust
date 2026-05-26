# @node-webrtc-rust/conference

TypeScript control-plane API for conference rooms with Rust-side audio mixing.

This package manages **who** is in a room, **mute state**, and **signaling wiring**. Audio capture, decode, mix, and encode run in the native `crates/conference` data plane — TypeScript never handles PCM or Opus buffers.

## Control plane vs data plane

| Layer | Package / crate | Responsibility |
| ----- | ---------------- | -------------- |
| Control plane | `@node-webrtc-rust/conference` | Room lifecycle, mute/kick admin APIs, signaling bridge |
| Signaling | `@node-webrtc-rust/signaling` | WebSocket SDP/ICE relay between browsers and Node |
| Data plane | `conference-bindings` → `crates/conference` | Peer connections, mixer graph, personalized output |

## Quick start

```typescript
import { createServer } from 'http'

import { ConferenceServer } from '@node-webrtc-rust/conference'
import { SignalingServer } from '@node-webrtc-rust/signaling'

const PORT = 3000
const httpServer = createServer()
const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
await signaling.listen(PORT)

const conference = new ConferenceServer()
conference.attachSignaling({ url: `ws://127.0.0.1:${PORT}/ws` })

conference.on('participant-joined', ({ roomId, participantId }) => {
  console.log(`joined ${participantId} in ${roomId}`)
})

await conference.createRoom('demo', {
  maxParticipants: 16,
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
})

// Serve static browser UI, REST admin routes, etc.
```

See [`examples/conference-room`](../../examples/conference-room/) for a full browser demo (when available).

## Mute modes

| Mode | API | Effect |
| ---- | --- | ------ |
| **Global mute** | `muteParticipant(id, { scope: 'global' })` | Target is excluded from **all** listeners' mixes |
| **Listener mute** | `muteParticipant(id, { scope: 'listener', listenerId })` | Only `listenerId` stops hearing `id`; others unchanged |
| **Room-wide silence** | `setMixingEnabled(false)` | Mixer outputs silence for everyone; per-participant mute state is preserved |

Use `unmuteParticipant` with the same scope to reverse a mute.

## Authentication (production)

v1 does not enforce roles in the library. Document and enforce policy in your app:

| Action | Recommended policy |
| ------ | ------------------- |
| `muteParticipant` with `scope: 'global'` | Admin / moderator only |
| `setMixingEnabled(false)` | Admin / moderator only |
| `kickParticipant` | Admin / moderator only |
| `muteParticipant` with `scope: 'listener'` | Owning client (local preference) |

Gate REST or RPC routes that call these methods before forwarding to `ConferenceRoom`.

## Events

`ConferenceServer` extends `EventEmitter` and emits:

- `room-created`, `room-destroyed`
- `participant-joined`, `participant-left`, `participant-kicked`, `participant-muted`
- `mixing-enabled-changed`
- `error`

Enable debug logging with `WEBRTC_DEBUG=1` to trace method calls and events.

## License

MIT
