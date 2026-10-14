import type { MixStats } from '@node-webrtc-rust/sdk/mix'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  ClientAudioMixer,
  type ClientMixGraph,
  type ClientMixerWarn,
} from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'

function createPassthroughGraph(): ClientMixGraph {
  const silence = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  const noop = () => undefined
  return {
    addInput: noop,
    removeInput: noop,
    pushFrame: noop,
    renderOutput: () => Buffer.from(silence),
    panTtsFrame: (pcm) => Buffer.from(pcm),
    setPose: noop,
    setPositionalEnabled: noop,
    setDefaultMixPlacement: noop,
    setTtsMixPlacement: noop,
    setTtsPose: noop,
    clearTtsPose: noop,
    setGroupMembers: noop,
    moveToGroup: noop,
    removeFromGroup: noop,
    mixStats: (): MixStats | null => null,
  }
}

/** TTS frame i carries marker i + 1 in its first int16 sample. */
function markedFrame(i: number): Buffer {
  const frame = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  frame.writeInt16LE(i + 1, 0)
  return frame
}

describe('ClientAudioMixer never drops TTS frames', () => {
  let mixer: ClientAudioMixer
  let warn: ReturnType<typeof vi.fn<ClientMixerWarn>>
  let tee: (...args: unknown[]) => void
  let markers: number[]
  const out = {
    writeSample: vi.fn(async (frame: Buffer) => {
      markers.push(frame.readInt16LE(0))
    }),
  }

  function burst(from: number, count: number): void {
    for (let i = from; i < from + count; i += 1) tee(markedFrame(i), 20)
  }

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'setInterval', 'Date', 'performance'] })
    markers = []
    out.writeSample.mockClear()
    warn = vi.fn<ClientMixerWarn>()
    mixer = new ClientAudioMixer({ graph: createPassthroughGraph(), warn })
    mixer.registerPeer('agent')
    mixer.wireTtsSidecar('agent', {
      setWriteSampleTee: vi.fn((cb: ((...args: unknown[]) => void) | null) => {
        if (cb) tee = cb
      }),
    } as never)
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('keeps every TTS frame when the pump stalls behind a burst', async () => {
    burst(0, 400)
    const nonZero = () => markers.filter((m) => m !== 0)
    for (let i = 0; i < 2000 && nonZero().length < 400; i += 1) {
      vi.advanceTimersByTime(20)
      await mixer.pumpMixFrame('agent', out)
    }
    expect(nonZero()).toEqual(Array.from({ length: 400 }, (_, i) => i + 1))
  })

  it('pump writes at most PUMP_MAX_FRAMES_PER_TICK frames per tick while draining a backlog', async () => {
    burst(0, 400)
    vi.advanceTimersByTime(1000)
    out.writeSample.mockClear()
    await mixer.pumpMixFrame('agent', out)
    expect(out.writeSample.mock.calls.length).toBeLessThanOrEqual(4)
  })

  it('warns once per window on a TTS backlog with depth fields', () => {
    burst(0, 120)
    expect(warn).toHaveBeenCalledTimes(1)
    const [message, fields] = warn.mock.calls[0]!
    expect(message.startsWith('[mix] tts backlog')).toBe(true)
    expect(fields).toMatchObject({ peerId: 'agent' })
    expect((fields as { depthMs: number }).depthMs).toBeGreaterThan(1000)

    burst(120, 60)
    vi.advanceTimersByTime(5000)
    burst(180, 1)
    expect(warn).toHaveBeenCalledTimes(1)

    vi.advanceTimersByTime(10_001)
    burst(181, 60)
    expect(warn).toHaveBeenCalledTimes(2)
  })

  it('discardQueuedTts drops the queued reply after barge-in', async () => {
    burst(0, 100)
    expect(mixer.discardQueuedTts('agent')).toBe(100)
    for (let i = 0; i < 10; i += 1) {
      vi.advanceTimersByTime(20)
      await mixer.pumpMixFrame('agent', out)
    }
    expect(markers.length).toBeGreaterThan(0)
    expect(markers.every((m) => m === 0)).toBe(true)
    expect(mixer.discardQueuedTts('unknown')).toBe(0)
  })
})
