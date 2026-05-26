# node-webrtc-rust

A Rust-backed native Node.js module providing browser-compatible WebRTC APIs with audio/video mixing capabilities. Built with [NAPI-RS](https://napi.rs) and [webrtc-rs](https://github.com/webrtc-rs/webrtc).

Unlike standalone media servers (Mediasoup, LiveKit), this is an **importable native module** — no separate infrastructure required.

## Features (v0.1.0 target)

- Browser-compatible `RTCPeerConnection` API
- Full ICE support (STUN + TURN)
- DataChannels (ordered/unordered, text + binary)
- Audio track sending and receiving
- WebSocket-based signaling helpers
- Prebuilt binaries for macOS, Linux, and Windows (no Rust toolchain needed for users)

## Packages

This is a monorepo containing:

| Package | Description |
|---------|-------------|
| `@node-webrtc-rust/sdk` | High-level TypeScript API mirroring the W3C WebRTC spec |
| `@node-webrtc-rust/bindings` | NAPI-RS native addon (compiled Rust → `.node` binary) |
| `@node-webrtc-rust/signaling` | Optional WebSocket signaling server/client helpers |

## Quick Start

```bash
npm install @node-webrtc-rust/sdk @node-webrtc-rust/signaling
```

```typescript
import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { SignalingServer, SignalingClient, autoNegotiate } from '@node-webrtc-rust/signaling'

const pc = new RTCPeerConnection({
  iceServers: [
    { urls: 'stun:stun.l.google.com:19302' },
    { urls: 'turn:your-server:3478', username: 'user', credential: 'pass' }
  ]
})

const dc = pc.createDataChannel('chat')
dc.onopen = () => dc.send('Hello!')
dc.onmessage = (event) => console.log('Received:', event.data)
```

## Supported Platforms

| OS | Arch | Status |
|----|------|--------|
| macOS | arm64 (M1+) | Supported |
| macOS | x64 (Intel) | Supported |
| Linux | x64 (glibc) | Supported |
| Linux | x64 (musl/Alpine) | Supported |
| Linux | arm64 (glibc) | Supported |
| Windows | x64 (MSVC) | Supported |

## Repository Structure

```
node-webrtc-rust/
├── Cargo.toml                    # Rust workspace manifest
├── package.json                  # npm workspace root
├── crates/
│   ├── core/                     # Pure Rust WebRTC engine (webrtc-rs wrapper)
│   ├── mixer/                    # Audio/video mixing pipeline (v0.2+)
│   └── signaling/                # Optional Rust signaling server (v0.2+)
├── packages/
│   ├── bindings/                 # NAPI-RS native addon + platform packages
│   │   ├── npm/                  # Per-platform prebuilt binary packages
│   │   └── src/                  # Rust NAPI binding code
│   ├── sdk/                      # TypeScript SDK (what users import)
│   └── signaling/                # Node.js signaling helpers
├── examples/                     # Runnable demo applications
├── .github/workflows/            # CI: cross-compile + test + publish
└── development/                  # Dev notes & plans (git-ignored)
```

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
# Install JS dependencies
npm install

# Build the native addon (current platform)
cd packages/bindings && npm run build

# Build the TypeScript packages
npm run build
```

### Running tests

```bash
# Rust tests
cargo test --workspace

# Node.js tests
npm test
```

## Roadmap

- **v0.1.0** — PeerConnection, DataChannels, audio tracks, STUN/TURN, signaling helpers
- **v0.2.0** — Video tracks, audio mixing, video compositing
- **v0.3.0** — Rust-side signaling server, statistics API, simulcast

## License

MIT
