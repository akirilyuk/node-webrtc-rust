# Conference Room — Manual Signaling

Same browser conference demo as [`conference-room`](../conference-room/), but **signaling is implemented by hand** — this example does not import `@node-webrtc-rust/signaling`.

Use it when you already have a signaling stack (Socket.IO, Redis pub/sub, your own WebSocket server) and want to see exactly how browser SDP/ICE maps into native conference DTOs.

## What is manual here?

| Piece                | Standard `conference-room`                           | This example                                                                                 |
| -------------------- | ---------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| WebSocket room relay | `SignalingServer` from `@node-webrtc-rust/signaling` | `ManualSignalingServer` in [`src/manual-signaling.ts`](src/manual-signaling.ts)              |
| Conference bridge    | `ConferenceServer.attachSignaling()` + SDK bridge    | Inline in `ManualSignalingServer` — calls `ConferenceRoom.handleSignalingMessage()` directly |
| Browser client       | Same wire protocol                                   | Same `public/client.js` (unchanged)                                                          |

The wire protocol matches `@node-webrtc-rust/signaling`:

- `join` — enter a room (`room`, `peerId`)
- `offer` / `answer` / `ice-candidate` — targeted SDP/ICE relay (`targetPeerId`)
- `peer-joined` / `peer-left` — room membership notifications

Messages addressed to `conference-server` are translated into native join/offer/answer/ice DTOs. Outbound offers from the mixer are sent back to browsers as `offer` messages from `conference-server`.

## Prerequisites

From the repo root:

```bash
npm install
npm run build:ts
cd packages/bindings && npm run build:local
```

## Run

```bash
npm run start --workspace=@node-webrtc-rust/example-conference-room-manual-signaling
```

Open `http://localhost:8081` in two or more browser tabs (default port **8081** so it can run alongside the standard conference demo on 8080).

## Key server code

```typescript
import { ConferenceServer } from '@node-webrtc-rust/sdk/conference'
import { ManualSignalingServer } from './manual-signaling'

const conference = new ConferenceServer()
const signaling = new ManualSignalingServer(httpServer, { path: '/ws' })
signaling.attachConference(conference)
```

When a browser sends `join`, `ManualSignalingServer`:

1. Adds the peer to the in-memory room map and broadcasts `peer-joined`.
2. Calls `room.handleSignalingMessage(JSON.stringify({ type: 'join', participantId, roomId }))`.
3. Forwards any returned SDP offer to the browser as a signaling `offer` from `conference-server`.

See [`src/manual-signaling.ts`](src/manual-signaling.ts) for the full relay and bridge logic.

## Manual test script

Same as [`conference-room/README.md`](../conference-room/README.md): join room `demo` in two tabs, verify exclude-self mixing, try mute/kick/mixing controls.

## License

node-webrtc-rust is source-available under the [Business Source License 1.1](https://github.com/akirilyuk/node-webrtc-rust/blob/main/LICENSE), starting with version 0.14.0.

- You can build, run and sell your own applications and agents with it, commercially, in production.
- You cannot run a hosting platform for other people's applications or agents built with node-webrtc-rust (an "Agent Hosting Service") without a commercial license. Contact hello@voicethere.com for one.
- Using it through the VoiceThere platform is always allowed, including building and reselling services that run on VoiceThere.
- Each version converts to Apache License 2.0 four years after it is published.
- Versions 0.13.4 and earlier were published under the MIT License and remain MIT.

The LICENSE file is the binding text; this summary is not. Vendored third-party code keeps its own licenses, see THIRD_PARTY_NOTICES.md in the repository.
