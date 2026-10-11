import { AudioMixGraph } from '@node-webrtc-rust/sdk/mix'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ClientAudioMixer } from '../src/client-audio-mixer.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../src/pcm.js'

function nativeAvailable(): boolean {
  try {
    const graph = new AudioMixGraph()
    graph.addInput('probe')
    graph.removeInput('probe')
    return true
  } catch {
    return false
  }
}

/** One stereo frame whose every sample is `value` (a per-frame marker). */
function constantFrame(value: number): Buffer {
  const frame = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  for (let i = 0; i < frame.length; i += 2) frame.writeInt16LE(value, i)
  return frame
}

describe.skipIf(!nativeAvailable())('ClientAudioMixer pump renders at its slot time', () => {
  let mixer: ClientAudioMixer
  let heard: number[]
  const out = {
    writeSample: vi.fn(async (data: Uint8Array | Buffer) => {
      heard.push(Buffer.from(data).readInt16LE(0))
    }),
  }

  beforeEach(async () => {
    vi.useFakeTimers({
      toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date', 'performance'],
    })
    heard = []
    out.writeSample.mockClear()
    const graph = new AudioMixGraph()
    mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('speaker')
    mixer.registerPeer('listener')
    graph.setListenerSources('listener', ['speaker'])
    for (let i = 0; i < 16; i++) graph.pushFrame('speaker', constantFrame(1_000 * (i + 1)))
    mixer.startMixPump('listener', out)
    // Drive the ticks by hand: stop the interval but keep the pump clock.
    await mixer.pauseMixPump('listener')
  })

  afterEach(() => {
    mixer.stopMixPump('listener')
    vi.useRealTimers()
  })

  /** Heard markers after the start-up silence; per-frame gain is constant, so equal steps = consecutive frames. */
  function audible(): number[] {
    return heard.filter((v) => v !== 0)
  }

  function expectConsecutive(values: number[]): void {
    expect(values.length).toBeGreaterThanOrEqual(2)
    const step = values[1]! - values[0]!
    expect(step).toBeGreaterThan(0)
    for (let i = 1; i < values.length; i++) {
      expect(values[i]! - values[i - 1]!, JSON.stringify(values)).toBe(step)
    }
  }

  it('steady 20 ms ticks play consecutive frames', async () => {
    for (let i = 0; i < 8; i++) {
      vi.advanceTimersByTime(PCM_FRAME_DURATION_MS)
      await mixer.pumpMixFrame('listener', out)
    }
    expect(audible().length).toBeGreaterThanOrEqual(6)
    expectConsecutive(audible())
  })

  it('a catch-up double render yields two different consecutive frames, never a repeat', async () => {
    for (let i = 0; i < 3; i++) {
      vi.advanceTimersByTime(PCM_FRAME_DURATION_MS)
      await mixer.pumpMixFrame('listener', out)
    }
    const before = heard.length
    // The timer stalls for two slots, then one pump call renders both back-to-back.
    vi.advanceTimersByTime(2 * PCM_FRAME_DURATION_MS)
    await mixer.pumpMixFrame('listener', out)
    expect(heard.length - before).toBe(2)
    for (let i = 0; i < 3; i++) {
      vi.advanceTimersByTime(PCM_FRAME_DURATION_MS)
      await mixer.pumpMixFrame('listener', out)
    }
    expect(audible().length).toBeGreaterThanOrEqual(6)
    expectConsecutive(audible())
  })
})
