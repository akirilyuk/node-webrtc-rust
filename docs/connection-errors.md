# Connection errors and root error handling

Libraries in this repo (`@node-webrtc-rust/sdk`, `@node-webrtc-rust/signaling`, and
`@voicethere/client`) bubble connection failures through a **single optional root
handler**. Each error is a {@link ConnectionError} with structured `source` metadata so
you do not need to attach `.on('error')` on every `SignalingClient`, `RTCPeerConnection`,
`RTCDataChannel`, or voice session.

## Quick start

```typescript
import { setRootConnectionErrorHandler } from '@node-webrtc-rust/sdk'
// or: import { setRootConnectionErrorHandler } from '@voicethere/client/browser'

setRootConnectionErrorHandler((error) => {
  const { source } = error
  if (source.subsystem === 'session') {
    analytics.track('session_error', { code: source.code, sessionId: source.sessionId })
    return
  }
  if (source.subsystem === 'signaling') {
    console.warn('signaling', source.phase, source.room, error.message)
    return
  }
  if (source.subsystem === 'webrtc') {
    console.warn('webrtc', source.kind, source.label, error.message)
  }
})
```

Clear the handler when your process or test suite shuts down:

```typescript
setRootConnectionErrorHandler(undefined)
```

## `ConnectionError.source`

| `subsystem` | Emitted by | `source` fields |
|-------------|------------|-----------------|
| `signaling` | `SignalingClient`, Node/browser signaling WebSocket | `phase` (`connect` \| `socket`), optional `room`, `peerId`, `url` |
| `session` | `@voicethere/client` provisioning / WebRTC / runner `session_error` DC | `sessionId`, `code`, optional `projectId`, `buildId` |
| `webrtc` | `RTCPeerConnection`, `RTCDataChannel`, browser voice signaling WS | `kind` (see below), optional `sessionId`, `peerId`, `label` |

### WebRTC `kind` values

| `kind` | Meaning |
|--------|---------|
| `peer-connection` | `RTCPeerConnection` entered `failed` or native state callback error |
| `ice` | ICE transport `failed` |
| `disconnect` | `RTCPeerConnection.close()` native error |
| `datachannel` | `RTCDataChannel` send/close/native error |
| `signaling-ws` | Browser voice session signaling WebSocket error |
| `connect` | Reserved for connect-timeout style failures |

Use {@link ConnectionError.is} to narrow:

```typescript
import { ConnectionError } from '@node-webrtc-rust/sdk'

client.on('error', (error: Error) => {
  if (ConnectionError.is(error)) {
    console.log(error.source.subsystem, error.message)
  }
})
```

## Recommended patterns

### 1. One root handler per app / test worker

Register **once** at process or worker startup. Let libraries bubble tagged errors
automatically. Keep per-session `onSessionError` when you need session-scoped UI (dashboard
toast, retry button) — it still runs **in addition** to the root handler.

```typescript
import { connectBrowserVoiceSession, setRootConnectionErrorHandler } from '@voicethere/client/browser'

setRootConnectionErrorHandler((error) => logger.error({ ...error.source, msg: error.message }))

const voice = await connectBrowserVoiceSession({
  credentials,
  onSessionError: (event) => showToast(event.code, event.message),
})
```

### 2. Per-instance listeners when you need local context

Root handler + instance listener is supported. `SignalingClient` and SDK emitters pass
`ConnectionError` instances (not raw `ECONNRESET` strings).

```typescript
signaling.on('error', (error) => {
  if (ConnectionError.is(error) && error.source.subsystem === 'signaling') {
  }
})
```

### 3. Tests: log and continue

In load/E2E harnesses, prefer catching turn-level failures instead of letting loopback or
signaling errors terminate the process. The root handler can log without rethrowing:

```typescript
setRootConnectionErrorHandler((error) => {
  e2eLogger.error(`${formatConnectionErrorSource(error.source)}: ${error.message}`)
})
```

### 4. Do not rely on unhandled `error` events

Node throws when an `EventEmitter` emits `error` with no listeners. SDK and signaling
call {@link dispatchConnectionError}, which:

1. Invokes the root handler (if set)
2. Re-emits on the instance when `.on('error')` listeners exist
3. Logs to stderr otherwise — **does not throw**

## API reference (exported from `@node-webrtc-rust/sdk`)

| Export | Role |
|--------|------|
| `setRootConnectionErrorHandler` | Install or clear the process-wide handler |
| `getRootConnectionErrorHandler` | Read current handler (tests) |
| `reportConnectionError` | Forward to root only; returns whether handled |
| `dispatchConnectionError` | Root → instance emit → stderr fallback |
| `createConnectionError` | Build a tagged error (libraries use this internally) |
| `formatConnectionErrorSource` | Stable log string from `source` |
| `ConnectionError` | Error subclass with `.source` and `.cause` |

`@node-webrtc-rust/signaling` re-exports the same symbols for convenience.

## `@voicethere/client` session errors

{@link emitSessionError} (used by `onSessionError`) **also** calls `reportConnectionError`
with `subsystem: 'session'`. Session error codes remain in `session-errors.ts`
(`WEBRTC_CONNECTION_FAILED`, `AGENT_CHILD_CRASHED`, …).

## Related

- `@voicethere/client` README — `onSessionError` examples
- Platform docs — `/docs/session-errors` on the staging dashboard
