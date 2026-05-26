# Examples

Runnable TypeScript demo applications for node-webrtc-rust.

Each example is an npm workspace package under this directory, authored in **TypeScript** and run with `tsx` or compiled output.

## Available examples

| Package                  | Description                                                            |
| ------------------------ | ---------------------------------------------------------------------- |
| **peer-connection**      | Two-peer WebRTC connection with a DataChannel over WebSocket signaling |
| **audio-cosine**         | Local audio track streaming a 440 Hz cosine tone generated in PCM      |
| **browser-cosine-chat**  | Browser clients receive a shared cosine tone from Node and chat in rooms |

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
connected peer in that room via WebRTC data channels.
