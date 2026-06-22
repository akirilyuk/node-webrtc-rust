# Signaling peer IDs (`client-*` convention)

Voice agents built with `@node-webrtc-rust/helpers` only negotiate WebRTC with browser (or Node) peers whose signaling **`peerId` starts with `client-`**. Any other prefix is ignored on the server — the client may join signaling successfully but **never receives an SDP offer**.

This document explains why, how to spot the failure, and how to customize the prefix safely.

---

## Quick rules

| Role | Peer id | Notes |
| ---- | ------- | ----- |
| **Browser / Node client** | `client-*` (required by default) | Omit `peerId` in `@voicethere/client` — default is `client-<random>` |
| **Voice agent server** | `voice-agent-server` | Constant `VOICE_AGENT_SERVER_PEER_ID` in helpers |
| **Custom apps** (cosine demo, mesh) | Your choice | Only if you **do not** use `VoiceAgentSessionHost` / `SessionPod` voice path |

**Bad (no offer):** `steady-1`, `user-tab2`, `churn-worker-3`  
**Good:** `client-steady-1`, `client-tab2`, `client-churn-3-c1`

---

## Why the prefix exists

`VoiceAgentSessionHost` listens for `peer-joined` on the signaling room. For each new peer it:

1. Skips the server peer (`voice-agent-server`)
2. Skips peers that do **not** match `clientPeerIdPrefix` (default **`client-`**)
3. Creates an RTCPeerConnection + `VoiceAgent` and sends an **offer** to matching peers

Relevant code: `packages/helpers/src/voice-agent-session-host.ts` (`peer-joined` handler).

`SessionPod` uses the same **`client-`** rule for idle-session teardown (cancel/restart grace when a client joins or leaves). Custom prefixes are not wired through `SessionPod` today — use the default prefix unless you only use `VoiceAgentSessionHost` directly and set `clientPeerIdPrefix` on both sides consistently.

---

## Symptom checklist (join works, WebRTC stuck)

When `peerId` does not match the server prefix:

| Layer | What you see |
| ----- | ------------ |
| **Session API** | `ready`, credentials returned (~normal provision time) |
| **Signaling** | WebSocket open, `join` sent, sometimes `rejoined` / `same_session_reconnect` |
| **WebRTC** | **No inbound `offer`**, `RTCPeerConnection` stays **`new`** until timeout |
| **Runner logs** | Room exists, **`connections=0`**, no `[voice client-…] offer sent` |

This is **not** TURN, signaling gateway saturation, or provisioning failure — it is the server deliberately ignoring the peer id.

**2026-06-20 example:** staging load test used `steady-3-c1` / `churn-2-c1`; voice-smoke passed with default `client-<random>` on the same project.

---

## Client side (`@voicethere/client`)

`connectBrowserVoiceSession` accepts optional `peerId`. When omitted:

```typescript
function defaultPeerId(): string {
  return `client-${Math.random().toString(36).slice(2, 10)}`;
}
```

For VoiceThere cloud/local voice sessions, **prefer the default** or pass an explicit `client-…` id. Keep the same `peerId` when using `reconnectPolicy: "same-session"`.

---

## Server side (helpers)

### Default (recommended)

```typescript
import { VoiceAgentSessionHost, VOICE_AGENT_SERVER_PEER_ID } from '@node-webrtc-rust/helpers'

// Clients join as client-* ; host joins as voice-agent-server
const host = new VoiceAgentSessionHost(signaling, iceServers, { voiceConfig })
```

### Custom prefix (advanced)

If your product uses a different client prefix, set **`clientPeerIdPrefix`** on the host **and** use the same prefix on every client:

```typescript
const PREFIX = 'myapp-client-'

const host = new VoiceAgentSessionHost(signaling, iceServers, {
  voiceConfig,
  clientPeerIdPrefix: PREFIX,
})

// Client must join as myapp-client-tab1, not client-tab1
```

Do **not** change only one side. `SessionPod` idle teardown still checks hardcoded `client-` until aligned — prefer default `client-` for pod-based runners.

---

## E2E and load tests

- **voice-smoke:** uses default client `peerId` — correct reference path.
- **load-staging:** must use `client-${workerId}-c${cycle}` (see `e2e/suites/load-staging/session-cycle.ts`).
- **Harness guard:** `e2e/suites/lib/voice-session-connect.ts` logs a warning if a custom `peerId` omits the `client-` prefix.

Operator runbook: [`e2e/docs/LOAD-STAGING.md`](../../e2e/docs/LOAD-STAGING.md) (troubleshooting table).

---

## Related

| Resource | Topic |
| -------- | ----- |
| [`packages/helpers/README.md`](../packages/helpers/README.md) | SessionPod, VoiceAgentSessionHost API |
| [`examples/voice-agent-local-sherpa-multi-client/README.md`](../examples/voice-agent-local-sherpa-multi-client/README.md) | Multi-tab `client-tab*` pattern |
| [`examples/browser-cosine-chat/README.md`](../examples/browser-cosine-chat/README.md) | Non-voice mesh (different peer rules) |
