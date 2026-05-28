import { afterAll, beforeAll, describe, expect, test } from 'vitest'

import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { LocalAudioTrack, RTCPeerConnection } from '../src'
import type { RemoteAudioTrack } from '../src/RemoteAudioTrack'
import { VoiceAgent } from '../src/voice'
import { defaultIceConfig, waitForConnection } from './helpers'

describe('VoiceAgent e2e', () => {
  let server: SignalingServer

  beforeAll(async () => {
    server = new SignalingServer({ port: 0 })
    await server.listen(0)
  })

  afterAll(async () => {
    await server.close()
  })

  test('loopback PC with mock voice agent TTS injection', async () => {
    const pc1 = new RTCPeerConnection(defaultIceConfig)
    const pc2 = new RTCPeerConnection(defaultIceConfig)

    const agentOut = new LocalAudioTrack('agent-audio', 'agent-stream')
    await pc1.addTrack(agentOut)

    const sig1 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'voice-e2e',
      peerId: 'agent',
    })
    const sig2 = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'voice-e2e',
      peerId: 'user',
    })

    autoNegotiate({ pc: pc1, signaling: sig1, polite: false })
    autoNegotiate({ pc: pc2, signaling: sig2, polite: true })

    let inbound: RemoteAudioTrack | undefined
    pc2.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        inbound = event.track as RemoteAudioTrack
      }
    }

    await sig1.connect()
    await sig2.connect()
    await waitForConnection(pc1)
    await waitForConnection(pc2)

    await agentOut.writeSample(Buffer.alloc(960), 5)

    expect(inbound).toBeDefined()
    const agent = new VoiceAgent({
      stt: { provider: 'mock', language: 'en' },
      tts: { provider: 'mock' },
      events: { mode: 'both' },
    })

    await agent.attach({ inboundTrack: inbound!, outboundTrack: agentOut })
    await agent.start()
    await agent.sendTextToTTS('Hello loopback')
    await agent.stop()

    pc1.close()
    pc2.close()
    sig1.disconnect()
    sig2.disconnect()
  })
})
