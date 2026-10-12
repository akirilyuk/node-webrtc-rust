/**
 * The server offer must carry the data-channel m-line even when the native
 * `createDataChannel` completes late.
 *
 * `RTCPeerConnection.createDataChannel` is fire-and-forget (the native call runs on the tokio
 * runtime and the SDK does not await it). The host calls `createOffer` right after, so on a
 * starved host the offer can be built before the native side counted the channel and the SDP has
 * no `m=application` section: ICE and DTLS connect, the control channel never opens (CI run
 * 38202583312, offer sdp_len 1135 instead of 1335 = exactly one application section less).
 *
 * This test delays the native create by 150 ms to force that ordering deterministically.
 */
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { RTCDataChannel, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import { VoiceSessionBudget, resetProcessVoiceSessionBudget } from '../src/voice-session-budget.js'
import { defaultIceConfig } from './mix-three-client-helpers.js'

type PcWithNative = RTCPeerConnection & {
  native: {
    createDataChannel(label: string, options?: unknown): Promise<unknown>
  }
}

describe.skipIf(typeof RTCPeerConnection !== 'function')(
  'host offer carries the data-channel section when native createDataChannel is late',
  () => {
    let server: SignalingServer
    let wsUrl: string
    let pod: SessionPod
    const original = RTCPeerConnection.prototype.createDataChannel
    const sessionBudget = new VoiceSessionBudget(1)
    let nativeCreateDelayMs: number | null = 150

    beforeAll(async () => {
      resetProcessVoiceSessionBudget()
      server = new SignalingServer({ port: 0 })
      await server.listen(0, '127.0.0.1')
      wsUrl = `ws://127.0.0.1:${server.port}`
      pod = new SessionPod(server, {
        signalingUrl: wsUrl,
        iceServers: defaultIceConfig.iceServers,
        sessionMode: 'voice+data',
        teardownIdleSessions: false,
        voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } },
        sessionBudget,
        maxPreparedSessions: 2,
      })
      RTCPeerConnection.prototype.createDataChannel = function (
        this: RTCPeerConnection,
        label: string,
        options?: RTCDataChannelInit,
      ) {
        const native = (this as PcWithNative).native
        const late =
          nativeCreateDelayMs === null
            ? new Promise(() => {}) // never settles
            : new Promise((resolve) => setTimeout(resolve, nativeCreateDelayMs!)).then(() =>
                native.createDataChannel(label, options),
              )
        return RTCDataChannel.fromNativePromise(late as never, label, options)
      }
    })

    afterAll(async () => {
      RTCPeerConnection.prototype.createDataChannel = original
      await pod.close().catch(() => undefined)
      resetProcessVoiceSessionBudget()
    })

    it('a native create that never settles fails the setup after 5 s and releases the slot', async () => {
      // Budget is 1: the next test only gets a slot if the failed setup released this one.
      nativeCreateDelayMs = null
      await pod.ensureSession('session-offer-dc-stuck')
      const client = new SignalingClient({
        url: wsUrl,
        room: 'session-offer-dc-stuck',
        peerId: 'client-offer-dc-stuck',
      })
      let offered = false
      client.on('offer', () => {
        offered = true
      })
      try {
        await client.connect()
        const deadline = Date.now() + 15_000
        while (Date.now() < deadline) {
          await new Promise((resolve) => setTimeout(resolve, 100))
          if (Date.now() > deadline - 8_000 && sessionBudget.snapshot().active === 0) break
        }
        expect(offered).toBe(false)
        expect(sessionBudget.snapshot().active).toBe(0)
      } finally {
        client.disconnect()
      }
    }, 30_000)

    it('offer includes the data channel when the native create is late', async () => {
      nativeCreateDelayMs = 150
      await pod.ensureSession('session-offer-dc')
      const client = new SignalingClient({
        url: wsUrl,
        room: 'session-offer-dc',
        peerId: 'client-offer-dc',
      })
      const offerSdp = new Promise<string>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('no offer within 10 s')), 10_000)
        client.on('offer', ({ sdp }: { sdp: { sdp?: string } }) => {
          clearTimeout(timer)
          resolve(sdp.sdp ?? '')
        })
      })
      try {
        await client.connect()
        const sdp = await offerSdp
        expect(sdp).toMatch(/m=application .*webrtc-datachannel/)
      } finally {
        client.disconnect()
      }
    }, 30_000)
  },
)
