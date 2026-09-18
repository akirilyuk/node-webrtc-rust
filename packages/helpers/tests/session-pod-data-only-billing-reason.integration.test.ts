/**
 * SessionPod data-only billing endReason integration — regression for helpers #243 leftover.
 *
 * Join/connect must clear the prepare `never_connected` pending reason so post-connect
 * idle teardown is not mislabeled (orchestrator zeros billable_seconds).
 *
 * Verify:
 *   cd node-webrtc-rust
 *   npm run test:integration --workspace=@node-webrtc-rust/helpers -- \
 *     packages/helpers/tests/session-pod-data-only-billing-reason.integration.test.ts
 */
import { afterEach, describe, expect, it } from 'vitest'

import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { VOICE_CONTROL_CHANNEL_LABEL } from '@node-webrtc-rust/sdk/voice'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod, type SessionPodChangeEvent } from '../src/session-pod.js'
import { defaultIceConfig, delay, waitForConnection } from './mix-three-client-helpers.js'

const REJOIN_GRACE_MS = 400
const NEVER_CONNECTED_GRACE_MS = 500
/** Long enough that a joined session cannot expire on the prepare timer during test 1. */
const PREPARE_NEVER_CONNECTED_GRACE_MS = 30_000

function sessionPodDataOnlyIntegrationNativeAvailable(): boolean {
  return typeof RTCPeerConnection === 'function'
}

type ConnectedDataClient = {
  pc: RTCPeerConnection
  signaling: SignalingClient
  teardownNegotiate: () => void
}

async function connectDataOnlyClient(
  wsUrl: string,
  sessionId: string,
  peerId: string,
): Promise<ConnectedDataClient> {
  const pc = new RTCPeerConnection(defaultIceConfig)
  const controlOpen = new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () =>
        reject(new Error(`timed out waiting for ${VOICE_CONTROL_CHANNEL_LABEL} open (${peerId})`)),
      20_000,
    )
    pc.ondatachannel = (event) => {
      if (event.channel.label !== VOICE_CONTROL_CHANNEL_LABEL) return
      if (event.channel.readyState === 'open') {
        clearTimeout(timer)
        resolve()
        return
      }
      event.channel.onopen = () => {
        clearTimeout(timer)
        resolve()
      }
    }
  })
  const signaling = new SignalingClient({
    url: wsUrl,
    room: sessionId,
    peerId,
  })
  const teardownNegotiate = autoNegotiate({ pc, signaling, polite: true })
  await signaling.connect()
  await waitForConnection(pc)
  await controlOpen
  return { pc, signaling, teardownNegotiate }
}

function closeDataClient(client: ConnectedDataClient): void {
  client.teardownNegotiate()
  client.pc.close()
  client.signaling.disconnect()
}

async function waitForSessionConnections(
  pod: SessionPod,
  sessionId: string,
  minConnections: number,
  timeoutMs = 20_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const info = pod.listSessions().find((session) => session.sessionId === sessionId)
    if (info && info.connections >= minConnections) return
    await delay(50)
  }
  throw new Error(`timed out waiting for ${minConnections} connection(s) on ${sessionId}`)
}

async function waitForDestroyed(
  events: SessionPodChangeEvent[],
  sessionId: string,
  timeoutMs = 60_000,
): Promise<SessionPodChangeEvent> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const match = events.find(
      (event) => event.action === 'destroyed' && event.sessionId === sessionId,
    )
    if (match) return match
    await delay(50)
  }
  throw new Error(`timed out waiting for destroyed event on ${sessionId}`)
}

describe.skipIf(!sessionPodDataOnlyIntegrationNativeAvailable())(
  'SessionPod data-only billing endReason integration',
  () => {
    let server: SignalingServer | undefined
    let wsUrl: string

    afterEach(async () => {
      if (server) {
        await server.close().catch(() => undefined)
        server = undefined
      }
    })

    it('post-connect idle teardown endReason is not never_connected (regression)', async () => {
      server = new SignalingServer({ port: 0 })
      await server.listen(0)
      wsUrl = `ws://127.0.0.1:${server.port}`

      const destroyedEvents: SessionPodChangeEvent[] = []
      const sessionId = 'session-data-billing-regression'
      const pod = new SessionPod(server, {
        signalingUrl: wsUrl,
        iceServers: defaultIceConfig.iceServers,
        voiceConfig: {} as never,
        sessionMode: 'data-only',
        teardownIdleSessions: true,
        rejoinGraceMs: REJOIN_GRACE_MS,
        neverConnectedRejoinGraceMs: PREPARE_NEVER_CONNECTED_GRACE_MS,
        onSessionChange: (event) => {
          if (event.action === 'destroyed') destroyedEvents.push(event)
        },
      })

      await pod.ensureSession(sessionId)
      const client = await connectDataOnlyClient(wsUrl, sessionId, 'client-billing-a')
      await waitForSessionConnections(pod, sessionId, 1)

      closeDataClient(client)
      await waitForDestroyed(destroyedEvents, sessionId)

      const destroyed = destroyedEvents.find((event) => event.sessionId === sessionId)
      expect(destroyed).toBeDefined()
      expect(destroyed!.endReason).not.toBe('never_connected')

      await pod.close().catch(() => undefined)
      server = undefined
    }, 60_000)

    it('never-joined prepare slot still destroys with never_connected (control)', async () => {
      server = new SignalingServer({ port: 0 })
      await server.listen(0)
      wsUrl = `ws://127.0.0.1:${server.port}`

      const destroyedEvents: SessionPodChangeEvent[] = []
      const sessionId = 'session-data-never-joined'
      const pod = new SessionPod(server, {
        signalingUrl: wsUrl,
        iceServers: defaultIceConfig.iceServers,
        voiceConfig: {} as never,
        sessionMode: 'data-only',
        teardownIdleSessions: true,
        neverConnectedRejoinGraceMs: NEVER_CONNECTED_GRACE_MS,
        onSessionChange: (event) => {
          if (event.action === 'destroyed') destroyedEvents.push(event)
        },
      })

      await pod.ensureSession(sessionId)
      await delay(NEVER_CONNECTED_GRACE_MS + 200)
      await waitForDestroyed(destroyedEvents, sessionId)

      const destroyed = destroyedEvents.find((event) => event.sessionId === sessionId)
      expect(destroyed).toBeDefined()
      expect(destroyed!.endReason).toBe('never_connected')

      await pod.close().catch(() => undefined)
      server = undefined
    })
  },
)
