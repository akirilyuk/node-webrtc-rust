# voice-agent-echo-pod

**Direct layer** — `SessionPod` + inline echo handler, **no runner**, **no `@voicethere/agent` child**.

Same voice semantics as [`e2e/fixtures/echo-agent`](../../../e2e/fixtures/echo-agent/agent.ts):

- `ready` TTS when a client connects
- `echo:{text}` on `user_speech_final` and chat messages

## Stack comparison

| Layer | Process | Agent logic | Redis |
|-------|---------|-------------|-------|
| **echo-pod** (this) | SessionPod + VoiceAgentSessionHost | Inline `echoVoiceHandler` | No |
| **runner M1** | Runner parent + child IPC | `e2e/fixtures/echo-agent` bundle | Yes |
| **staging** | K8s runner pod | Deployed echo bundle | Yes |

Use this to isolate bugs in **WebRTC + Sherpa STT/TTS + SessionPod reconnect** before involving runner IPC or agent child.

## Run

```bash
cd node-webrtc-rust
npm run download-stt:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
npm run build:native
npm run start --workspace=@node-webrtc-rust/example-voice-agent-echo-pod
```

Default **port 8090** (`ECHO_POD_PORT`). Sets `WEBRTC_NAT_1TO1_IPS=127.0.0.1` automatically.

## E2E (headless full TTS/STT)

```bash
# Terminal 1 — echo pod (above)

# Terminal 2
cd e2e
npm run download-sherpa-models
npm run test:local-direct-voice-reconnect
npm run test:local-direct-voice-load   # concurrent workers + reconnect churn
```

## API (runner-compatible subset)

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/healthz` | Liveness |
| `POST` | `/api/sessions` | `{ "sessionId": "..." }` |
| `GET` | `/api/sessions` | Active sessions |
| `WS` | `/ws` | Signaling (plain, no JWT) |

No `/readyz` (no Redis).
