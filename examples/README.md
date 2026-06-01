# Examples

Runnable TypeScript demo applications for node-webrtc-rust.

Each example is an npm workspace package under this directory, authored in **TypeScript** and run with `tsx` (no separate compile step).

**Voice STT/TTS vendor API docs:** [`shared/VOICE_VENDOR_REFERENCE.md`](./shared/VOICE_VENDOR_REFERENCE.md)

**Free local STT + TTS:** we recommend [`voice-agent-local-sherpa`](./voice-agent-local-sherpa/README.md) (`local-sherpa`) when you want **privacy** (mic audio not sent to third-party speech APIs) and **lower latency** (no cloud STT/TTS round-trips). Download open-weight Sherpa models once — no speech API keys.

## Available examples

| Package                                   | Type                    | Default port     | Description                                                                                                                                                                                     |
| ----------------------------------------- | ----------------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **peer-connection**                       | CLI (exits on success)  | 8080 (signaling) | Two Node peers, DataChannel over WebSocket signaling                                                                                                                                            |
| **peer-connection** `start:parity`        | CLI (exits on success)  | 8080             | Transceivers, `getStats`, `setConfiguration`, `replaceTrack`, `readSample` tour                                                                                                                 |
| **audio-cosine**                          | CLI (runs ~5s)          | 8080 (signaling) | Local audio track streaming a 440 Hz cosine tone in PCM                                                                                                                                         |
| **audio-cosine** `start:replace-track`    | CLI (runs ~4s)          | 8080             | `replaceTrack` + `RemoteAudioTrack.readSample` with 440→880 Hz swap                                                                                                                             |
| **browser-cosine-chat**                   | Browser + Node server   | 3000             | Browser tabs hear a server cosine tone and mesh chat via data channels                                                                                                                          |
| **conference-room**                       | Browser + Node server   | 8080             | Browser mic → Rust mixer → personalized mixed audio; mute/kick UI                                                                                                                               |
| **conference-room-manual-signaling**      | Browser + Node server   | 8081             | Same as conference-room; hand-rolled WebSocket signaling                                                                                                                                        |
| **voice-agent** `start:callback`          | CLI (exits on success)  | 8080 (signaling) | Mock VoiceAgent with callback speech events                                                                                                                                                     |
| **voice-agent** `start:stream`            | CLI (exits on success)  | 8080             | Mock VoiceAgent with `speechEvents()` stream                                                                                                                                                    |
| **voice-agent** `start:barge-in`          | CLI (exits on success)  | 8080             | Barge-in flush when VAD detects inbound speech                                                                                                                                                  |
| **voice-agent** `start:live:*`            | CLI (exits on success)  | 8080             | Per-vendor live manual test (API keys; see `voice-agent/README.md`)                                                                                                                             |
| **voice-agent-browser**                   | Browser + Node server   | 3001             | Browser mic → STT events via DataChannel; client triggers TTS + barge-in demo                                                                                                                   |
| **voice-agent-browser** `start:live:*`    | Browser + Node server   | 3001             | Same UI with live cloud STT/TTS (`VOICE_VENDOR` + API keys; see README)                                                                                                                         |
| **voice-agent-local-sherpa**              | Browser + Node server   | 3002             | Sherpa browser demo + **7 CI roundtrip E2E** scripts — [`ROUNDTRIP.md`](./voice-agent-local-sherpa/ROUNDTRIP.md) (`roundtrip`, `roundtrip-counting`, `roundtrip-utterance-timing`, `roundtrip-two-phrases`, `roundtrip-counting-echo`, `roundtrip-counting-barge-recovery`, `roundtrip-barge-in`) |
| **voice-agent-local-sherpa-multi-client** | Browser + Node (3 tabs) | 3004             | **Three clients, one room**; edit `src/voice-handler.ts` for STT/TTS logic; shared Sherpa pool + session budget — [README](./voice-agent-local-sherpa-multi-client/README.md)                   |
| **voice-agent-multi-session-pod**         | Browser + Node server   | 3003             | One pod, many sessions via `@node-webrtc-rust/helpers` `SessionPod`                                                                                                                             |

## Run examples locally

### Prerequisites

- **Node.js** ≥ 18 and **npm** ≥ 9
- **[Rust](https://rustup.rs)** (stable) — required to build the native `.node` addon
- **Network** — examples use public STUN (`stun.l.google.com:19302`); no TURN required for local demos
- **Browser examples** — Chrome, Firefox, or Safari with microphone permission (conference demos only)

### One-time setup (from repo root)

```bash
npm run setup
```

This runs, in order:

1. `npm run install:all` — install root + all workspace deps (`packages/*`, `examples/*`)
2. `npm run build:native` — build the host debug `.node` in `packages/bindings/`
3. `npm run build:ts` — compile `@node-webrtc-rust/sdk` and `@node-webrtc-rust/signaling`

Verify the native binding loads:

```bash
node -e "require('./packages/bindings').version()"
```

### Start an example

Always run from the **repo root**:

```bash
npm run start --workspace=@node-webrtc-rust/example-<name>
```

Replace `<name>` with `peer-connection`, `audio-cosine`, `browser-cosine-chat`, `conference-room`, or `conference-room-manual-signaling`.

**Run one example at a time.** Several CLI demos bind signaling on port **8080**; only one process can listen. Stop the previous example (`Ctrl+C`) before starting the next, or override the port where supported (`PORT=8090 npm run start --workspace=...` for browser servers).

Optional debug logging:

```bash
WEBRTC_DEBUG=1 npm run start --workspace=@node-webrtc-rust/example-conference-room
```

### Quick reference

| Example                          | Command                                                                                                     | How to verify                                                                                             |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| peer-connection                  | `npm run start --workspace=@node-webrtc-rust/example-peer-connection`                                       | Prints `Received: Hello from peer 1!` and exits                                                           |
| peer-connection parity           | `npm run start:parity --workspace=@node-webrtc-rust/example-peer-connection`                                | Runs three parity scenarios; prints `All parity scenarios completed`                                      |
| audio-cosine                     | `npm run start --workspace=@node-webrtc-rust/example-audio-cosine`                                          | Logs remote track + streams tone for ~5s, then exits                                                      |
| audio-cosine replace             | `npm run start:replace-track --workspace=@node-webrtc-rust/example-audio-cosine`                            | Swaps 440→880 Hz via `replaceTrack`; logs `readSample` byte lengths                                       |
| browser-cosine-chat              | `npm run start --workspace=@node-webrtc-rust/example-browser-cosine-chat`                                   | Open `http://localhost:3000`, same room in multiple tabs                                                  |
| conference-room                  | `npm run start --workspace=@node-webrtc-rust/example-conference-room`                                       | Open `http://localhost:8080`, join room, allow mic                                                        |
| conference-room-manual-signaling | `npm run start --workspace=@node-webrtc-rust/example-conference-room-manual-signaling`                      | Open `http://localhost:8081` (see its README)                                                             |
| voice-agent callback             | `npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent`                                  | Prints callback speech events and exits                                                                   |
| voice-agent stream               | `npm run start:stream --workspace=@node-webrtc-rust/example-voice-agent`                                    | Prints stream events from mock TTS                                                                        |
| voice-agent barge-in             | `npm run start:barge-in --workspace=@node-webrtc-rust/example-voice-agent`                                  | Logs barge-in after simulated user speech                                                                 |
| voice-agent live OpenAI          | `OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent`         | Live vendor demo; see `examples/voice-agent/README.md`                                                    |
| voice-agent live (any)           | `npm run start:live:deepgram` / `elevenlabs` / `cartesia` / `assemblyai` / `google`                         | Same pattern with vendor env vars                                                                         |
| voice-agent-browser              | `npm run start --workspace=@node-webrtc-rust/example-voice-agent-browser`                                   | Open `http://localhost:3001`, connect, speak, use TTS form and barge-in button                            |
| voice-agent-browser live OpenAI  | `OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent-browser` | Live STT/TTS via browser mic + DataChannel; see `voice-agent-browser/README.md`                           |
| voice-agent-browser live (any)   | `start:live:deepgram` / `elevenlabs` / `cartesia` / `assemblyai` / `google`                                 | Set `VOICE_VENDOR` + vendor env vars; full table in README                                                |
| voice-agent-local-sherpa         | `download-stt:en` + `download-tts:en`; `bash scripts/ci/run-sherpa-example-ci.sh e2e` (all roundtrips) | Seven Sherpa E2E scripts in CI Test job — [ROUNDTRIP.md](./voice-agent-local-sherpa/ROUNDTRIP.md#ci-github-actions) |
| voice-agent-multi-session-pod    | `npm run start --workspace=@node-webrtc-rust/example-voice-agent-multi-session-pod`                         | Open `http://localhost:3003`, different session IDs in multiple tabs; `GET /api/sessions` for pod metrics |

### Troubleshooting

| Problem                            | Fix                                                                              |
| ---------------------------------- | -------------------------------------------------------------------------------- |
| `Failed to load native binding`    | Run `npm run build:native` from repo root                                        |
| `EADDRINUSE` on example ports      | Re-run `npm run start` — servers call [`shared/free-port.ts`](./shared/free-port.ts) to stop the previous listener on that port. Set `VOICE_SKIP_FREE_PORT=1` to disable. Manual: `lsof -ti :3004 \| xargs kill` |
| Browser page loads but no audio    | Allow microphone (conference demos); check browser console; try `WEBRTC_DEBUG=1` |
| After changing Rust or TS packages | `npm run build:native && npm run build:ts` then restart the example              |

---

## Running an example (details)

### DataChannel demo

```bash
npm run start --workspace=@node-webrtc-rust/example-peer-connection
```

Set `WEBRTC_DEBUG=1` to trace WebRTC and signaling calls (see the root README).

### WebRTC parity APIs (v0.2)

Commented CLI tours for transceivers, statistics, configuration, and media lifecycle:

```bash
npm run start:parity --workspace=@node-webrtc-rust/example-peer-connection
npm run start:replace-track --workspace=@node-webrtc-rust/example-audio-cosine
```

Shared helpers and PCM notes: [`examples/shared/`](shared/).

Browser demo: open `http://localhost:3000?debug` to log ICE/signaling state and `getStats` RTP counters.

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
