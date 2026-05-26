# Conference Room Example

Browser clients send microphone audio to a Rust-side conference mixer and receive a
**personalized mixed stream** (other participants only — no self-audio).

## Prerequisites

From the repo root:

```bash
npm install
npm run build:ts
cd packages/bindings && npm run build:local
```

## Run

```bash
npm run start --workspace=@node-webrtc-rust/example-conference-room
```

Open `http://localhost:8080` in two or more browser tabs. Enter the same room name (default
`demo`), allow microphone access, and click **Connect**.

## Debug logging

Trace conference, signaling, and native WebRTC calls:

```bash
WEBRTC_DEBUG=1 npm run start --workspace=@node-webrtc-rust/example-conference-room
```

Server logs include `mixing-enabled-changed`, `participant-muted`, and `participant-kicked`
events. Native bindings emit `[webrtc-debug]` lines when `WEBRTC_DEBUG=1` (see root README).

## Manual test script

1. Open tab A and tab B in room `demo`.
2. Speak on tab A — tab B hears A; tab A does not hear itself.
3. On tab A, click **Mute for me** on tab B — A stops hearing B; B still hears A.
4. Click **Disable room mixing** — both tabs hear silence.
5. Click **Enable room mixing** — audio returns.
6. Kick tab B — tab B disconnects.

## API imports

```typescript
import { ConferenceServer } from '@node-webrtc-rust/sdk/conference'
import { SignalingServer } from '@node-webrtc-rust/signaling'
```

Conference mute modes, events, and production auth guidance: [`packages/sdk/README.md`](../../packages/sdk/README.md).
