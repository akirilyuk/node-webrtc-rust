# node-webrtc-rust

[![Build](https://github.com/node-webrtc-rust/node-webrtc-rust/actions/workflows/build.yml/badge.svg)](https://github.com/node-webrtc-rust/node-webrtc-rust/actions/workflows/build.yml)

A Rust-backed native Node.js module providing browser-compatible WebRTC APIs with audio/video mixing capabilities. Built with [NAPI-RS](https://napi.rs) and [webrtc-rs](https://github.com/webrtc-rs/webrtc).

Unlike standalone media servers (Mediasoup, LiveKit), this is an **importable native module** — no separate infrastructure required.

## Features (v0.1.0)

- Browser-compatible `RTCPeerConnection` API
- Full ICE support (STUN + TURN)
- DataChannels (ordered/unordered, text + binary)
- Audio track sending and receiving
- WebSocket-based signaling helpers
- Prebuilt binaries for macOS, Linux, and Windows (no Rust toolchain needed for users)

## Packages

| Package                       | Description                                             |
| ----------------------------- | ------------------------------------------------------- |
| `@node-webrtc-rust/sdk`       | High-level TypeScript API mirroring the W3C WebRTC spec |
| `@node-webrtc-rust/bindings`  | NAPI-RS native addon (compiled Rust → `.node` binary)   |
| `@node-webrtc-rust/signaling` | Optional WebSocket signaling server/client helpers      |

## Quick Start

```bash
npm install @node-webrtc-rust/sdk @node-webrtc-rust/signaling
```

```typescript
import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { SignalingServer, SignalingClient, autoNegotiate } from '@node-webrtc-rust/signaling'

// Start signaling server
const server = new SignalingServer({ port: 8080 })
await server.listen()

// Peer 1 — creates the data channel and initiates negotiation
const pc1 = new RTCPeerConnection({
  iceServers: [
    { urls: 'stun:stun.l.google.com:19302' },
    { urls: 'turn:your-turn-server:3478', username: 'user', credential: 'pass' },
  ],
})
const sig1 = new SignalingClient({ url: 'ws://localhost:8080', room: 'demo' })
autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
await sig1.connect()

const dc = pc1.createDataChannel('chat')
dc.onopen = () => dc.send('Hello from Peer 1!')

// Peer 2 — answers and receives the data channel
const pc2 = new RTCPeerConnection({
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
})
const sig2 = new SignalingClient({ url: 'ws://localhost:8080', room: 'demo' })
autoNegotiate({ pc: pc2, signaling: sig2, polite: true })
await sig2.connect()

pc2.ondatachannel = (event) => {
  event.channel.onmessage = (msg) => console.log('Received:', msg.data)
}
```

See [`examples/peer-connection/`](examples/peer-connection/) for a runnable demo.

## Supported Platforms

| OS      | Arch              | Status    |
| ------- | ----------------- | --------- |
| macOS   | arm64 (M1+)       | Supported |
| macOS   | x64 (Intel)       | Supported |
| Linux   | x64 (glibc)       | Supported |
| Linux   | x64 (musl/Alpine) | Supported |
| Linux   | arm64 (glibc)     | Supported |
| Windows | x64 (MSVC)        | Supported |

## Architecture

```
┌──────────────────────────────────────────┐
│            Node.js (Control Plane)       │  Business logic, signaling,
│         @node-webrtc-rust/sdk            │  authentication
└──────────────────────┬───────────────────┘
                       │ NAPI-RS
                       ▼
┌──────────────────────────────────────────┐
│          Rust Core Engine (Data Plane)   │  WebRTC stack, ICE, RTP,
│           crates/core + crates/mixer     │  audio/video processing
└──────────────────────────────────────────┘
```

## Development

### Prerequisites

- Rust toolchain (stable) via [rustup](https://rustup.rs)
- Node.js >= 18
- npm >= 9

### Building from source

```bash
npm install
cd packages/bindings && npm run build:local   # host-only — fast local dev
npm run build
```

### Running tests

```bash
# Rust integration tests
cargo test -p node-webrtc-rust-core

# TypeScript unit + E2E tests
npm test

# TURN integration test (requires coturn)
docker compose -f docker-compose.test.yml up -d
TURN_AVAILABLE=1 npm test --workspace=@node-webrtc-rust/sdk
docker compose -f docker-compose.test.yml down
```

## Roadmap

- **v0.1.0** — PeerConnection, DataChannels, audio tracks, STUN/TURN, signaling helpers
- **v0.2.0** — Video tracks, audio mixing, video compositing
- **v0.3.0** — Rust-side signaling server, statistics API, simulcast

## License

MIT
