# Examples

Runnable TypeScript demo applications for node-webrtc-rust.

Each example is an npm workspace package under this directory, authored in **TypeScript** and run with `tsx` or compiled output.

## Available examples

- **peer-connection** — Basic two-peer WebRTC connection with WebSocket signaling

## Running an example

```bash
npm install
npm run build --workspace=@node-webrtc-rust/sdk --workspace=@node-webrtc-rust/signaling
cd packages/bindings && npm run build
npm run start --workspace=@node-webrtc-rust/example-peer-connection
```
