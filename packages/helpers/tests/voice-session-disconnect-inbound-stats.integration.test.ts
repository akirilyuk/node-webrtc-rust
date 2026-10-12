/**
 * VoiceSessionContext.getInboundAudioStats from onPeerDisconnected with real audio.
 *
 * A voice session receives real mic frames over a native loopback, then the client
 * disconnects. The disconnect hook must see the counters read just before close.
 *
 * Verify:
 *   cd node-webrtc-rust
 *   npm run test:integration --workspace=@node-webrtc-rust/helpers -- \
 *     tests/voice-session-disconnect-inbound-stats.integration.test.ts
 */
import { afterEach, describe, expect, it } from 'vitest'

import { LocalAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import { resetProcessVoiceSessionBudget, VoiceSessionBudget } from '../src/voice-session-budget.js'
import type { InboundAudioStats } from '../src/inbound-audio-stats.js'
import type { VoiceSessionContext } from '../src/voice-session-handler.js'
import { pumpLoudMicFrames } from './mix-energy-helpers.js'
import { defaultIceConfig, delay, waitForConnection } from './mix-three-client-helpers.js'

function nativeAvailable(): boolean {
  return typeof RTCPeerConnection === 'function'
}

async function waitFor<T>(
  read: () => Promise<T | undefined> | T | undefined,
  what: string,
  timeoutMs = 20_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const value = await read()
    if (value !== undefined) return value
    await delay(25)
  }
  throw new Error(`timed out waiting for ${what}`)
}

describe.skipIf(!nativeAvailable())(
  'VoiceSessionContext.getInboundAudioStats from onPeerDisconnected (integration)',
  () => {
    let server: SignalingServer | undefined
    let pod: SessionPod | undefined

    afterEach(async () => {
      await pod?.close().catch(() => undefined)
      pod = undefined
      if (server) {
        await server.close().catch(() => undefined)
        server = undefined
      }
      resetProcessVoiceSessionBudget()
    })

    it('reports packets and bytes received before close', async () => {
      resetProcessVoiceSessionBudget()
      server = new SignalingServer({ port: 0 })
      await server.listen(0, '127.0.0.1')
      const wsUrl = `ws://127.0.0.1:${server.port}`
      const sessionId = 'session-inbound-audio-disconnect'

      let connectedCtx: VoiceSessionContext | undefined
      let disconnectStats: InboundAudioStats | null | undefined
      let disconnectSettled = false
      pod = new SessionPod(server, {
        signalingUrl: wsUrl,
        iceServers: defaultIceConfig.iceServers,
        sessionMode: 'voice+data',
        teardownIdleSessions: false,
        voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } },
        sessionBudget: new VoiceSessionBudget(2),
        voiceHandler: {
          onPeerConnected: (ctx) => {
            connectedCtx = ctx
          },
          onPeerDisconnected: async (ctx) => {
            disconnectStats = await ctx.getInboundAudioStats()
            disconnectSettled = true
          },
        },
      })
      await pod.ensureSession(sessionId)

      const pc = new RTCPeerConnection(defaultIceConfig)
      const mic = new LocalAudioTrack('stats-mic', 'stream-stats')
      await pc.addTrack(mic)
      const signaling = new SignalingClient({
        url: wsUrl,
        room: sessionId,
        peerId: 'client-stats-audio',
      })
      const teardownNegotiate = autoNegotiate({ pc, signaling, polite: true })
      await signaling.connect()
      await waitForConnection(pc)

      let pumping = true
      const pump = (async () => {
        while (pumping) {
          await pumpLoudMicFrames((frame, ms) => mic.writeSample(frame, ms), 200)
        }
      })()

      try {
        const ctx = await waitFor(() => connectedCtx, 'onPeerConnected')
        // Live read: wait until real audio has arrived server-side.
        await waitFor(async () => {
          const live = await ctx.getInboundAudioStats({ timeoutMs: 2_000 })
          return live && live.packetsReceived > 0 ? live : undefined
        }, 'live inbound audio')
      } finally {
        pumping = false
        await pump
      }

      teardownNegotiate()
      pc.close()
      signaling.disconnect()

      await waitFor(() => (disconnectSettled ? true : undefined), 'onPeerDisconnected stats')
      expect(disconnectStats).not.toBeNull()
      expect(disconnectStats!.packetsReceived).toBeGreaterThan(0)
      expect(disconnectStats!.bytesReceived).toBeGreaterThan(0)
    }, 60_000)
  },
)
