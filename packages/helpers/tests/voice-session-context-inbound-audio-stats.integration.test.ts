/**
 * VoiceSessionContext.getInboundAudioStats against a real peer connection.
 *
 * A data-only session never receives audio, so the context must resolve `null` while the
 * peer is connected (no inbound audio stream) and after it closes, without rejecting.
 *
 * Verify:
 *   cd node-webrtc-rust
 *   npm run test:integration --workspace=@node-webrtc-rust/helpers -- \
 *     tests/voice-session-context-inbound-audio-stats.integration.test.ts
 */
import { afterEach, describe, expect, it } from 'vitest'

import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import type { VoiceSessionContext } from '../src/voice-session-handler.js'
import { defaultIceConfig, delay, waitForConnection } from './mix-three-client-helpers.js'

function nativeAvailable(): boolean {
  return typeof RTCPeerConnection === 'function'
}

async function waitFor<T>(read: () => T | undefined, what: string, timeoutMs = 20_000): Promise<T> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const value = read()
    if (value !== undefined) return value
    await delay(25)
  }
  throw new Error(`timed out waiting for ${what}`)
}

describe.skipIf(!nativeAvailable())(
  'VoiceSessionContext.getInboundAudioStats (integration)',
  () => {
    let server: SignalingServer | undefined

    afterEach(async () => {
      if (server) {
        await server.close().catch(() => undefined)
        server = undefined
      }
    })

    it('resolves null for a connected data-only peer and for the disconnect hook context', async () => {
      server = new SignalingServer({ port: 0 })
      await server.listen(0, '127.0.0.1')
      const wsUrl = `ws://127.0.0.1:${server.port}`
      const sessionId = 'session-inbound-audio-stats'

      let connectedCtx: VoiceSessionContext | undefined
      let disconnectedCtx: VoiceSessionContext | undefined
      const pod = new SessionPod(server, {
        signalingUrl: wsUrl,
        iceServers: defaultIceConfig.iceServers,
        voiceConfig: {} as never,
        sessionMode: 'data-only',
        teardownIdleSessions: false,
        voiceHandler: {
          onPeerConnected: (ctx) => {
            connectedCtx = ctx
          },
          onPeerDisconnected: (ctx) => {
            disconnectedCtx = ctx
          },
        },
      })

      await pod.ensureSession(sessionId)

      const pc = new RTCPeerConnection(defaultIceConfig)
      const signaling = new SignalingClient({
        url: wsUrl,
        room: sessionId,
        peerId: 'client-stats-a',
      })
      const teardownNegotiate = autoNegotiate({ pc, signaling, polite: true })
      await signaling.connect()
      await waitForConnection(pc)

      const ctx = await waitFor(() => connectedCtx, 'onPeerConnected')
      await expect(ctx.getInboundAudioStats({ timeoutMs: 2_000 })).resolves.toBeNull()

      teardownNegotiate()
      pc.close()
      signaling.disconnect()

      const closedCtx = await waitFor(() => disconnectedCtx, 'onPeerDisconnected')
      await delay(500)
      await expect(closedCtx.getInboundAudioStats({ timeoutMs: 2_000 })).resolves.toBeNull()

      await pod.close().catch(() => undefined)
    }, 60_000)
  },
)
