# Roadmap (outlook)

Planned direction for **node-webrtc-rust** — not release versions or dates. For what ships today, see the [README](README.md) and the [WebRTC API parity matrix](docs/webrtc-api-parity.md).

---

## Observability

### OpenTelemetry metrics

- **Metrics** across the voice and WebRTC stack, with configuration on both the **Rust** core and the **Node** SDK (enable/disable exporters, endpoints, resource attributes, session-scoped labels).
- Hooks aligned with cloud deployment (runner pods, Session Explorer) so operators can scrape or export OTLP without custom forks.

### OpenTelemetry verbosity

- **Separate verbosity levels for OpenTelemetry** — e.g. metrics-only vs. full spans, optional high-cardinality attributes — independent of application log noise and independent of Rust `VOICE_DEBUG`-style tracing.

### Log scopes

- **Scoped logging** instead of one global debug switch: distinct areas such as WebRTC ICE/SDP, RTP, voice pipeline (VAD/STT/TTS), conference mixing, and signaling helpers, each with its own level on Rust and Node.
- Goal: run production with quiet defaults while turning up only the subsystem you are debugging.

---

## WebRTC platform

### W3C WebRTC parity

- Continue closing gaps against **WebRTC 1.0** and browser behavior: Unified Plan edge cases, stats, video send/receive, simulcast / `RTCRtpSender` parameters, DTMF where telephony needs it, and remaining P2 items in the parity matrix.

Details and current status: [`docs/webrtc-api-parity.md`](docs/webrtc-api-parity.md).

### Video mixing

- **MCU-style video compositing** for conference rooms: multi-participant layouts and routing on top of the existing audio `MixGraph` / conference path (extension APIs, not a separate SFU product).

---

## Integrations

### Discord voice channels

- **Discord** support for joining and bridging **voice channels** (bot/gateway media path and signaling design TBD) so agents built on this stack can participate in Discord voice alongside browser and PSTN-style flows.

---

## How this doc stays current

When work ships, update the parity matrix and README feature sections; adjust or remove items here so this file stays an **outlook**, not a changelog.
