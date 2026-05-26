# Examples

Runnable TypeScript demo applications for node-webrtc-rust.

Each example is an npm workspace package under this directory, authored in **TypeScript** and run with `tsx` or compiled output.

## Available examples

| Package                  | Description                                                            |
| ------------------------ | ---------------------------------------------------------------------- |
| **peer-connection**      | Two-peer WebRTC connection with a DataChannel over WebSocket signaling |
| **audio-cosine**         | Local audio track streaming a 440 Hz cosine tone generated in PCM      |
| **browser-cosine-chat**  | Browser clients receive a shared cosine tone from Node and chat in rooms |
| **conference-room**      | Browser mic audio mixed in Rust with mute, mixing toggle, and kick controls |
| **conference-room-manual-signaling** | Same conference mixer; WebSocket signaling implemented by hand (no `@node-webrtc-rust/signaling`) |

## Prerequisites

Build the TypeScript packages and native bindings once:

```bash
npm install
npm run build:ts
cd packages/bindings && npm run build:local
```

## Running an example

### DataChannel demo

```bash
npm run start --workspace=@node-webrtc-rust/example-peer-connection
```

Set `WEBRTC_DEBUG=1` to trace WebRTC and signaling calls (see the root README).

### Audio track + cosine generator

Streams interleaved stereo 16-bit PCM at 48 kHz from a `CosineGenerator` through
`LocalAudioTrack.writeSample`. The receiver logs when the remote track arrives.

```bash
npm run start --workspace=@node-webrtc-rust/example-audio-cosine
```

Tune the tone in `examples/audio-cosine/src/index.ts` (`TONE_HZ`, `STREAM_SECONDS`).
Adjust waveform shape in `examples/audio-cosine/src/cosine-generator.ts`.

### Browser clients + cosine stream + room chat

Node runs the signaling server, HTTP static host, and a cosine tone broadcaster
that fans out the same 440 Hz tone to every browser tab in a room. Clients use
native browser `RTCPeerConnection` for audio from the server and mesh data
channels for room chat.

```bash
npm run start --workspace=@node-webrtc-rust/example-browser-cosine-chat
```

Open `http://localhost:3000` in multiple tabs, enter the same room name, and
click **Connect**. Each tab hears the cosine tone; messages propagate to every
connected peer in that room via WebRTC data channels. The page shows a live
waveform and spectrum graph of the incoming server audio track.

### Conference room (audio mixing)

Browser clients send microphone audio to a Rust-side mixer and receive a personalized
mixed stream (excluding self-audio). The demo exposes global mute, per-listener mute,
room-wide mixing on/off, and kick via REST admin routes.

```bash
npm run start --workspace=@node-webrtc-rust/example-conference-room
```

Open `http://localhost:8080` in multiple tabs, join the same room, and allow microphone
access. Live waveform graphs show your outgoing microphone and the incoming mixed
track from the Rust mixer. Set `WEBRTC_DEBUG=1` to trace conference and WebRTC calls. See
[`conference-room/README.md`](conference-room/README.md) for a manual test script.

### Conference room — manual signaling

Identical browser UI and Rust mixer, but the server implements WebSocket room relay and the
conference bridge inline in `src/manual-signaling.ts` instead of using
`@node-webrtc-rust/signaling` or `ConferenceServer.attachSignaling()`.

```bash
npm run start --workspace=@node-webrtc-rust/example-conference-room-manual-signaling
```

Runs on **port 8081** by default so it can coexist with the standard conference demo.
See [`conference-room-manual-signaling/README.md`](conference-room-manual-signaling/README.md).
