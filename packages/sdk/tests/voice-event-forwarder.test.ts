import { afterEach, describe, expect, test } from 'vitest'

import { LocalAudioTrack } from '../src'
import { VoiceAgent } from '../src/voice'
import { createVoiceLoopback } from './voice-helpers'

const FRAME_MS = 20
const FRAME_BYTES = 3840

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** 500 Hz stereo tone (whole cycles per 20 ms frame) at a level the energy VAD accepts. */
function toneFrame(): Buffer {
  const frame = Buffer.alloc(FRAME_BYTES)
  for (let i = 0; i < FRAME_BYTES / 4; i++) {
    const s = Math.round(12_000 * Math.sin((2 * Math.PI * 500 * i) / 48_000))
    frame.writeInt16LE(s, i * 4)
    frame.writeInt16LE(s, i * 4 + 2)
  }
  return frame
}

describe('VoiceAgent native speech event forwarder', () => {
  const cleanups: Array<() => Promise<void>> = []

  afterEach(async () => {
    for (const fn of cleanups.splice(0)) await fn().catch(() => undefined)
  })

  async function startVadAgent(mode: 'both' | 'stream' = 'both'): Promise<VoiceAgent> {
    const { userInbound, cleanup } = await createVoiceLoopback()
    cleanups.push(cleanup)
    const agent = new VoiceAgent({
      vad: { enabled: true, provider: 'energy', threshold: 0.05, minSpeechDurationMs: 100 },
      events: { mode },
    })
    await agent.attach({
      inboundTrack: userInbound,
      outboundTrack: new LocalAudioTrack('fwd-out', 'fwd'),
      nativeInbound: false,
    })
    await agent.start()
    cleanups.push(() => agent.stop())
    return agent
  }

  test('speech events are delivered after a 1.5 s JS block without loss', async () => {
    const agent = await startVadAgent()
    const types: string[] = []
    agent.on('speech', (event) => types.push(event.type))
    const native = agent.getNativeAgent()
    const tone = toneFrame()
    const silence = Buffer.alloc(FRAME_BYTES)

    const feed = async (frame: Buffer, ms: number): Promise<void> => {
      let anchor = performance.now()
      for (let n = 0; n < ms / FRAME_MS; n++) {
        await native.processInboundPcm(frame, FRAME_MS)
        const wait = anchor + (n + 1) * FRAME_MS - performance.now()
        if (wait > 0) await sleep(wait)
        else anchor = performance.now() - (n + 1) * FRAME_MS
      }
    }

    for (let cycle = 0; cycle < 5; cycle++) {
      await feed(tone, 400)
      if (cycle === 2) {
        // Block the JS thread mid-run; the native forwarder must keep every event.
        const end = Date.now() + 1_500
        while (Date.now() < end) {
          // busy wait
        }
      }
      await feed(silence, 1_500)
    }

    const starts = () => types.filter((t) => t === 'user_speaking_start').length
    const ends = () => types.filter((t) => t === 'user_speaking_end').length
    const deadline = Date.now() + 10_000
    while ((starts() < 5 || ends() < 5) && Date.now() < deadline) await sleep(20)

    expect(starts()).toBe(5)
    expect(ends()).toBe(5)
    const vad = types.filter((t) => t === 'user_speaking_start' || t === 'user_speaking_end')
    expect(vad).toEqual(
      Array.from({ length: 5 }, () => ['user_speaking_start', 'user_speaking_end']).flat(),
    )
  }, 60_000)

  test('stop resolves a pending speechEvents iterator', async () => {
    const agent = await startVadAgent()
    const iterator = agent.speechEvents()
    const pending = iterator.next()
    await sleep(100)
    const started = Date.now()
    await agent.stop()
    const result = await Promise.race([pending, sleep(500).then(() => 'timeout' as const)])
    expect(result).not.toBe('timeout')
    expect(Date.now() - started).toBeLessThan(500)
    expect((result as IteratorResult<unknown>).done).toBe(true)
  }, 30_000)

  test('nextSpeechEvent called after stop resolves null', async () => {
    const agent = await startVadAgent()
    await agent.stop()
    const result = await Promise.race([
      agent.getNativeAgent().nextSpeechEvent(),
      sleep(200).then(() => 'timeout' as const),
    ])
    expect(result).toBeNull()
  }, 30_000)

  test('events queued before stop are still delivered', async () => {
    const agent = await startVadAgent('stream')
    const native = agent.getNativeAgent()
    // Stream mode: no JS pull loop drains the queue in this test.
    const tone = toneFrame()
    for (let n = 0; n < 15; n++) await native.processInboundPcm(tone, FRAME_MS)
    await sleep(200)
    await agent.stop()
    const first = await Promise.race([native.nextSpeechEvent(), sleep(500).then(() => 'timeout')])
    expect(first).not.toBe('timeout')
    expect(first).not.toBeNull()
    // Drain anything else queued; then the queue is empty and the call resolves null.
    let next = await native.nextSpeechEvent()
    for (let i = 0; i < 20 && next; i++) next = await native.nextSpeechEvent()
    expect(next).toBeNull()
  }, 30_000)
})
