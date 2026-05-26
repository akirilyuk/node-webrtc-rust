# Conference Room Example

Browser clients send microphone audio to a Rust-side conference mixer and receive a
**personalized mixed stream** (other participants only — no self-audio).

## Prerequisites

From the repo root:

```bash
npm install
npm run build:ts
npm run build --workspace=@node-webrtc-rust/conference
cd packages/conference-bindings && npm run build:local
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
2. Speak on A — B hears A; A does **not** hear their own voice in the mix.
3. On tab A, click **Mute for me** on B — A stops hearing B; B still hears A.
4. Click **Global mute** on B — all listeners stop hearing B.
5. Click **Disable room mixing** — both tabs hear silence.
6. Click **Enable room mixing** — audio returns.
7. Click **Kick** on B — B disconnects.

## Controls

| Control | Effect |
| ------- | ------ |
| **Global mute / unmute** | Target excluded from (or restored to) **all** listeners' mixes |
| **Mute / unmute for me** | **Listener-scoped** mute — only this tab stops hearing the target |
| **Disable / enable room mixing** | Room-wide silence while preserving per-participant mute state |
| **Kick** | Removes participant from the conference room |

Admin REST routes (`/api/rooms/:room/mute`, `/mixing`, `/kick`) are unauthenticated in this
demo. Gate them with auth in production.
