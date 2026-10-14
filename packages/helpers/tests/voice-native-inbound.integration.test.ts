import { LocalAudioTrack, RTCPeerConnection, RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { PCM_FRAME_DURATION_MS } from '../src/pcm.js'
import { createLoudStereoFrame } from './mix-energy-helpers.js'
import { defaultIceConfig } from './mix-three-client-helpers.js'

const SEND_FRAMES = 100

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function waitConnected(pc: RTCPeerConnection): Promise<void> {
  if (pc.connectionState === 'connected') return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('connection timeout')), 20_000)
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'connected') {
        clearTimeout(timer)
        resolve()
      }
    }
  })
}

/** Two native peers on loopback: `sender` writes PCM, `inbound` is the receiving remote track. */
async function createLoopback(): Promise<{
  sender: LocalAudioTrack
  inbound: RemoteAudioTrack
  cleanup: () => Promise<void>
}> {
  const server = new SignalingServer({ port: 0 })
  await server.listen(0, '127.0.0.1')
  const senderPc = new RTCPeerConnection(defaultIceConfig)
  const receiverPc = new RTCPeerConnection(defaultIceConfig)
  const sender = new LocalAudioTrack('native-inbound-out', 'native-inbound')
  await senderPc.addTrack(sender)
  const url = `ws://127.0.0.1:${server.port}`
  const sigA = new SignalingClient({ url, room: 'native-inbound', peerId: 'sender' })
  const sigB = new SignalingClient({ url, room: 'native-inbound', peerId: 'receiver' })
  autoNegotiate({ pc: senderPc, signaling: sigA, polite: false })
  autoNegotiate({ pc: receiverPc, signaling: sigB, polite: true })
  const inboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for ontrack')), 20_000)
    receiverPc.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        clearTimeout(timer)
        resolve(event.track as RemoteAudioTrack)
      }
    }
  })
  await sigA.connect()
  await sigB.connect()
  await waitConnected(senderPc)
  await waitConnected(receiverPc)
  await sender.writeSample(Buffer.alloc(960), 5)
  const inbound = await inboundPromise
  return {
    sender,
    inbound,
    cleanup: async () => {
      senderPc.close()
      receiverPc.close()
      sigA.disconnect()
      sigB.disconnect()
      await server.close()
    },
  }
}

/** Real-time paced tone frames anchored at `anchor + n * 20 ms`. */
async function sendTone(sender: LocalAudioTrack, frames: number): Promise<void> {
  const frame = createLoudStereoFrame()
  const anchor = performance.now()
  for (let n = 0; n < frames; n++) {
    await sender.writeSample(frame, PCM_FRAME_DURATION_MS)
    const due = anchor + (n + 1) * PCM_FRAME_DURATION_MS
    const wait = due - performance.now()
    if (wait > 0) await sleep(wait)
  }
}

describe('native inbound PCM loop', () => {
  const cleanups: Array<() => Promise<void>> = []

  afterEach(async () => {
    vi.restoreAllMocks()
    delete process.env.NWR_NATIVE_INBOUND
    for (const fn of cleanups.splice(0)) await fn().catch(() => undefined)
  })

  async function setup(): Promise<{ sender: LocalAudioTrack; inbound: RemoteAudioTrack }> {
    const loop = await createLoopback()
    cleanups.push(loop.cleanup)
    return loop
  }

  function vadAgent(): VoiceAgent {
    return new VoiceAgent({
      vad: { enabled: true, provider: 'energy', threshold: 0.05, minSpeechDurationMs: 100 },
    })
  }

  it('native inbound feeds the agent', async () => {
    const { sender, inbound } = await setup()
    const readSample = vi.spyOn(RemoteAudioTrack.prototype, 'readSample')
    const agent = vadAgent()
    const events: string[] = []
    agent.on('speech', (event) => events.push(event.type))
    await agent.attach({ inboundTrack: inbound, outboundTrack: new LocalAudioTrack('o', 'o') })
    await agent.start()
    try {
      await sendTone(sender, SEND_FRAMES)
      await sleep(200)
    } finally {
      await agent.stop()
    }
    expect(readSample).not.toHaveBeenCalled()
    expect(events.some((t) => t === 'user_speaking_start' || t === 'vad_triggered')).toBe(true)
  }, 30_000)

  it('read tee receives every inbound frame in native mode', async () => {
    const { sender, inbound } = await setup()
    const readSample = vi.spyOn(RemoteAudioTrack.prototype, 'readSample')
    let teeFrames = 0
    inbound.setReadSampleTee(() => {
      teeFrames += 1
    })
    const agent = vadAgent()
    await agent.attach({ inboundTrack: inbound, outboundTrack: new LocalAudioTrack('o', 'o') })
    await agent.start()
    try {
      await sendTone(sender, SEND_FRAMES)
      await sleep(300)
    } finally {
      await agent.stop()
      inbound.setReadSampleTee(null)
    }
    expect(readSample).not.toHaveBeenCalled()
    expect(teeFrames).toBeGreaterThanOrEqual(90)
  }, 30_000)

  it('a JS readSample wrapper keeps the JS loop', async () => {
    const { sender, inbound } = await setup()
    const orig = inbound.readSample.bind(inbound)
    let wrapped = 0
    inbound.readSample = async () => {
      wrapped += 1
      return orig()
    }
    const agent = vadAgent()
    await agent.attach({ inboundTrack: inbound, outboundTrack: new LocalAudioTrack('o', 'o') })
    await agent.start()
    try {
      await sendTone(sender, 25)
      await sleep(100)
    } finally {
      await agent.stop()
    }
    expect(wrapped).toBeGreaterThan(0)
  }, 30_000)
})
