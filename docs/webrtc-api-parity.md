# WebRTC API parity

How closely `@node-webrtc-rust/sdk` matches the browser **WebRTC 1.0** APIs ([W3C](https://www.w3.org/TR/webrtc/), [MDN `RTCPeerConnection`](https://developer.mozilla.org/en-US/docs/Web/API/RTCPeerConnection)).

**Legend**

| Symbol | Meaning |
|--------|---------|
| ✅ | Supported for intended use cases |
| 🟡 | Partial — API exists but behavior/options differ from the spec |
| ❌ | Not implemented |
| ➕ | **Extension** — not in the W3C API (library-specific) |

**Scope:** Node.js server/embedded peers and conference MCU. We do **not** target full browser parity (no DOM, no `getUserMedia`, no WebCodecs render path). Goal is **predictable interoperability** with browser peers for audio + data channels, plus conference mixing.

---

## Summary

| Area | Parity | Notes |
|------|--------|-------|
| ICE / SDP negotiation | 🟡 ~80% | Offer/answer, trickle ICE, STUN/TURN; several config/offer options missing |
| Data channels | 🟡 ~70% | Send/receive; limited init options and backpressure events |
| Audio send | 🟡 ~60% | PCM push model, not `MediaStreamTrack` capture |
| Audio receive | 🟡 ~65% | `ontrack` + `RemoteAudioTrack.readSample()` (Opus → PCM) |
| Video | ❌ | Types exist; no local/remote video pipeline in SDK |
| RTP transceivers / simulcast | ❌ | Plan-style `addTrack` only |
| Statistics | 🟡 ~70% | `getStats()` returns webrtc-rs report as `Map`; no per-sender selector yet |
| `MediaDevices` | ❌ | Out of scope (Node has no device capture API) |
| Conference MCU | ➕ | Rust-side mixing — extension, not W3C |

---

## `RTCPeerConnection`

### Constructor & configuration

| API | Status | Notes |
|-----|--------|-------|
| `new RTCPeerConnection(config?)` | ✅ | |
| `config.iceServers` | ✅ | STUN/TURN URLs, username, credential |
| `config.iceTransportPolicy` | 🟡 | `'all'` \| `'relay'` passed to native |
| `config.bundlePolicy` | ❌ | |
| `config.rtcpMuxPolicy` | ❌ | |
| `config.iceCandidatePoolSize` | ❌ | |
| `config.certificates` | ❌ | |
| `config.debug` | ➕ | Enables `[webrtc-debug]` logging |
| `setConfiguration()` | ✅ | ICE servers + `iceTransportPolicy` |
| `getConfiguration()` | ✅ | Cached copy in SDK; updated on construct / `setConfiguration` |

### Session description & ICE

| API | Status | Notes |
|-----|--------|-------|
| `createOffer(options?)` | 🟡 | `iceRestart`, `offerToReceiveAudio`, `voiceActivityDetection`; `offerToReceiveVideo` errors (video N/A) |
| `createAnswer(options?)` | 🟡 | `voiceActivityDetection` |
| `setLocalDescription(desc)` | ✅ | |
| `setLocalDescription()` (implicit) | ❌ | Must pass explicit description |
| `setRemoteDescription(desc)` | ✅ | |
| `addIceCandidate(candidate)` | ✅ | |
| `addIceCandidate(null)` (end-of-candidates) | 🟡 | Use `onicecandidate` with `candidate: null`; explicit null add not documented |
| `localDescription` / `remoteDescription` getters | 🟡 | Cached in SDK after set*; refreshed after `gatheringComplete()` for local |
| `pendingLocalDescription` / `currentLocalDescription` | ❌ | |
| `signalingState` | ✅ | |
| `gatheringComplete()` | ➕ | Blocks until ICE gathering done; **not in W3C** — common pattern for non-trickle signaling |

### Connection state

| API | Status | Notes |
|-----|--------|-------|
| `connectionState` | ✅ | |
| `iceConnectionState` | ✅ | |
| `iceGatheringState` | ✅ | |
| `onconnectionstatechange` | ✅ | EventEmitter + property handler |
| `oniceconnectionstatechange` | ✅ | |
| `onicegatheringstatechange` | ✅ | |
| `onsignalingstatechange` | ✅ | |
| `onicecandidate` | ✅ | |
| `onicecandidateerror` | ❌ | |
| `canTrickleIceCandidates` | ❌ | |

### Media & transceivers

| API | Status | Notes |
|-----|--------|-------|
| `addTrack(track, ...streams)` | 🟡 | **`LocalAudioTrack` only**; stream args ignored |
| `removeTrack(sender)` | ✅ | Detaches send on the given {@link RTCRtpSender} |
| `addTransceiver(...)` | ❌ | |
| `getSenders()` | ❌ | Only handle returned from `addTrack` |
| `getReceivers()` | ❌ | |
| `getTransceivers()` | ❌ | |
| `ontrack` | 🟡 | See [Media tracks](#media-streams-and-tracks) |
| `onnegotiationneeded` | ✅ | |

### Data channels

| API | Status | Notes |
|-----|--------|-------|
| `createDataChannel(label, options?)` | 🟡 | See [RTCDataChannel](#rtcdatachannel) |
| `ondatachannel` | ✅ | |

### Lifecycle & advanced

| API | Status | Notes |
|-----|--------|-------|
| `close()` | ✅ | Fire-and-forget async native close |
| `restartIce()` | ✅ | Triggers native ICE restart + negotiation-needed |
| `getStats(selector?)` | ✅ | `selector` ignored; returns `Map` like browser `RTCStatsReport` |
| `sctp` / `iceTransport` / `dtlsTransport` | ❌ | No transport object exposure |
| `addEventListener` / `removeEventListener` | ✅ | Via Node `EventEmitter` |

---

## `RTCSessionDescription` & `RTCIceCandidate`

| API | Status | Notes |
|-----|--------|-------|
| `RTCSessionDescription` | ✅ | `type`, `sdp` |
| `RTCSessionDescriptionInit` | ✅ | Used in signaling helpers |
| `RTCIceCandidate` | ✅ | |
| `RTCIceCandidateInit` | ✅ | |
| `toJSON()` on candidates | 🟡 | Check signaling export shape when interoperating |

---

## Media streams and tracks

### Capture & devices (browser-only)

| API | Status | Notes |
|-----|--------|-------|
| `navigator.mediaDevices.getUserMedia` | ❌ | Use file/PCM/synth sources + `LocalAudioTrack.writeSample` |
| `navigator.mediaDevices.enumerateDevices` | ❌ | |
| `MediaRecorder` | ❌ | |

### `MediaStream`

| API | Status | Notes |
|-----|--------|-------|
| `new MediaStream()` | 🟡 | Created around remote tracks in `ontrack` |
| `getTracks()` / `getAudioTracks()` / `getVideoTracks()` | 🟡 | Minimal stub |
| `addTrack()` / `removeTrack()` | 🟡 | Limited wiring |
| `id` | ✅ | |

### `MediaStreamTrack`

| API | Status | Notes |
|-----|--------|-------|
| `id`, `kind`, `enabled` | ✅ | |
| `readyState` (`live` \| `ended`) | 🟡 | `stop()` sets ended locally only |
| `muted` | ❌ | |
| `onended` / `onmute` / `onunmute` | ❌ | |
| `clone()` | 🟡 | Shallow clone |
| `getSettings()` / `getCapabilities()` / `applyConstraints()` | ❌ | |
| **Remote decode in SDK** | ✅ | {@link RemoteAudioTrack.readSample} (Opus → 48 kHz stereo PCM) |

### `LocalAudioTrack` (library pattern)

| API | Status | Notes |
|-----|--------|-------|
| `writeSample(pcm, durationMs)` | ➕ | **48 kHz stereo PCM** → encoded to negotiated codec (Opus for WebRTC) |
| Prime frame (960 B / 5 ms) | ➕ | Required so remote `ontrack` fires — see SDK JSDoc |
| Video local track | ❌ | Roadmap v0.2.x |

### Receive path behavior (important)

Browser: `ontrack` → attach to `<audio>` or WebAudio.

**node-webrtc-rust:**

1. **`ontrack` fires only after the remote sender calls `writeSample` at least once** (first RTP packet). Negotiation alone is insufficient.
2. **`RemoteAudioTrack.readSample()`** decodes inbound Opus to 48 kHz stereo PCM for app-level processing. The conference crate also decodes internally for MCU mixing.

---

## `RTCRtpSender` & `RTCRtpReceiver`

| API | Status | Notes |
|-----|--------|-------|
| `RTCRtpSender.track` | ✅ | Updated by `replaceTrack` |
| `RTCRtpSender.replaceTrack(track)` | ✅ | Audio only; `null` detaches send |
| `RTCRtpSender.transport` | ❌ | |
| `RTCRtpSender.getParameters()` / `setParameters()` | ❌ | Simulcast/bitrate — roadmap v0.3 |
| `RTCRtpReceiver.track` | ❌ | No receiver objects; use `ontrack` |
| `RTCRtpReceiver.getStats()` | ❌ | |

---

## `RTCDataChannel`

| API | Status | Notes |
|-----|--------|-------|
| `label`, `ordered`, `protocol`, `id` | 🟡 | Init options partially forwarded |
| `maxPacketLifeTime` / `maxRetransmits` | 🟡 | In init type; verify native SCTP mapping |
| `negotiated` | 🟡 | |
| `readyState` | ✅ | |
| `send(string \| Buffer)` | ✅ | `ArrayBuffer` / `Uint8Array` coerced |
| `close()` | ✅ | |
| `binaryType` | 🟡 | Property only; incoming binary as `Buffer` |
| `bufferedAmount` | ✅ | Cached property synced from native after send and on low events |
| `bufferedAmountLowThreshold` | ✅ | Forwarded to SCTP stack |
| `onopen` / `onmessage` / `onclose` / `onerror` | ✅ | |
| `onbufferedamountlow` | ✅ | |
| `send()` before open | ➕ | Queued until native channel ready |

---

## Statistics & diagnostics

| API | Status | Notes |
|-----|--------|-------|
| `RTCPeerConnection.getStats()` | ✅ | JSON stats from webrtc-rs → SDK `Map` |
| `RTCStatsReport` types | ❌ | |
| `WEBRTC_DEBUG` / `config.debug` | ➕ | Call/event tracing |

---

## Conference API (`@node-webrtc-rust/sdk/conference`)

Not part of W3C WebRTC — **server-side MCU** for multi-participant audio.

| API | Status | Notes |
|-----|--------|-------|
| `ConferenceServer` / `ConferenceRoom` | ➕ ✅ | Room lifecycle |
| `attachSignalingBridge` | ➕ ✅ | JSON signaling over WS helper |
| Per-listener mix / exclude-self | ➕ ✅ | `MixGraph` in Rust |
| Global / per-listener mute | ➕ ✅ | |
| `setMixingEnabled` | ➕ ✅ | |
| `kickParticipant` | ➕ ✅ | |
| Explicit routing matrix (who hears whom) | ✅ | `MixGraph::set_listener_sources` allow-list per listener |
| Video compositing | ❌ | Roadmap v0.2.x |

---

## Signaling helpers (`@node-webrtc-rust/signaling`)

| API | Status | Notes |
|-----|--------|-------|
| `SignalingServer` | ➕ ✅ | Room-based SDP/ICE relay |
| `SignalingClient` | ➕ ✅ | |
| `autoNegotiate` | ➕ ✅ | Wires two `RTCPeerConnection`s |

---

## Recommended parity roadmap

Prioritized for **browser interop** and **your conference product**:

### P0 — interoperability

1. ~~**`RTCRtpSender.replaceTrack()`**~~ — done (v0.2.x)
2. ~~**`removeTrack()`**~~ — done (v0.2.x)
3. ~~**`createOffer` / `createAnswer` options**~~ — `iceRestart` + `offerToReceiveAudio` done; video receive N/A
4. ~~**Remote audio decode in SDK**~~ — `RemoteAudioTrack.readSample()` done

### P1 — operability

5. ~~**`getStats()`**~~ — RTT, packets lost, jitter via webrtc-rs collector
6. ~~**`onicegatheringstatechange` / `onsignalingstatechange`**~~ — done
7. ~~**Data channel `bufferedAmount` + `onbufferedamountlow`**~~ — done
8. ~~**`setConfiguration()`**~~ — ICE servers + transport policy; `getConfiguration()` cached in SDK

### P2 — advanced WebRTC (deferred — v0.3+ / out of scope for audio-first Node peers)

9. **`addTransceiver` / Unified Plan controls** — requires `RTCRtpTransceiver` surface
10. **Simulcast / encodings** — v0.3 roadmap
11. **Video** — `LocalVideoTrack`, H.264/VP8 send, remote receive
12. **DTMF** — telephony milestone

### P3 — conference-specific (extensions, deferred)

13. ~~**Routing matrix API** on `MixGraph`~~ — `set_listener_sources` / `clear_listener_routes`
14. **Load / latency benchmarks** — criterion + simulated N-participant rooms (deferred)

---

## Testing parity

When adding APIs, cover:

1. **Browser ↔ Node** — offer/answer with Chrome/Firefox against SDK peer
2. **Node ↔ Node** — existing Vitest/e2e in `packages/sdk/tests`
3. **Conference** — `crates/conference/tests`, browser demos
4. **Regression** — document behavior changes in this file

---

## Related docs

- [SDK README](../packages/sdk/README.md)
- [CI / release](../scripts/ci/README.md)
- [Repository roadmap](../README.md#roadmap)
