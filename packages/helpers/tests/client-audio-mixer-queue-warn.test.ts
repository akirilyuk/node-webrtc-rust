import type { MixStats } from '@node-webrtc-rust/sdk/mix'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ClientAudioMixer, type ClientMixGraph } from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'

function zeroStats(): MixStats {
  return {
    pushed: 0,
    played: 0,
    compressed: 0,
    concealed: 0,
    overflowCompressed: 0,
    decodeErrors: 0,
    depth: 2,
    target: 3,
  }
}

/** Minimal fake graph: only what the pump tick touches, plus mutable per-source stats. */
function createStatsGraph(stats: Map<string, MixStats>): ClientMixGraph {
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
    mixStats: (id) => stats.get(id) ?? null,
  }
}

describe('ClientAudioMixer mix queue warnings', () => {
  let stats: Map<string, MixStats>
  let warn: ReturnType<typeof vi.fn<(message: string) => void>>
  let mixer: ClientAudioMixer
  const out = { writeSample: vi.fn(async () => undefined) }

  /** Advances the fake clock by `ms` and runs one pump tick for the listener. */
  async function tick(ms: number): Promise<void> {
    vi.advanceTimersByTime(ms)
    await mixer.pumpMixFrame('listener', out)
  }

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'setInterval', 'Date', 'performance'] })
    stats = new Map([['speaker', zeroStats()]])
    warn = vi.fn<(message: string) => void>()
    mixer = new ClientAudioMixer({ graph: createStatsGraph(stats), warn })
    mixer.registerPeer('speaker')
    mixer.registerPeer('listener')
    mixer.startMixPump('listener', out)
    out.writeSample.mockClear()
  })

  afterEach(() => {
    mixer.stopMixPump('listener')
    vi.useRealTimers()
  })

  it('passes mixStats through to the graph', () => {
    stats.get('speaker')!.compressed = 4
    expect(mixer.mixStats('speaker')?.compressed).toBe(4)
    expect(mixer.mixStats('nobody')).toBeNull()
  })

  it('warns once with peer, deltas, depth and target when a counter increases', async () => {
    await tick(1000)
    expect(warn).not.toHaveBeenCalled()

    const s = stats.get('speaker')!
    s.played = 40
    s.compressed = 3
    s.concealed = 1
    s.depth = 7
    s.target = 4
    await tick(1000)

    expect(warn).toHaveBeenCalledTimes(1)
    const line = warn.mock.calls[0]![0]
    expect(line).toContain('peerId=speaker')
    expect(line).toContain('compressed=+3')
    expect(line).toContain('concealed=+1')
    expect(line).toContain('overflowCompressed=+0')
    expect(line).toContain('decodeErrors=+0')
    expect(line).toContain('depth=7')
    expect(line).toContain('target=4')
  })

  it('does not warn about start-up concealment before the source played its first frame', async () => {
    const s = stats.get('speaker')!
    s.concealed = 6
    await tick(1000)
    await tick(1000)
    expect(warn).not.toHaveBeenCalled()

    // Once audio has played, the start-up concealment stays forgiven but new concealment warns.
    s.played = 25
    await tick(1000)
    expect(warn).not.toHaveBeenCalled()
    s.concealed = 8
    await tick(1000)
    expect(warn).toHaveBeenCalledTimes(1)
    expect(warn.mock.calls[0]![0]).toContain('concealed=+2')
  })

  it('stays silent for a fresh silent peer that never played a frame', async () => {
    for (let sec = 0; sec < 15; sec++) await tick(1000)
    expect(warn).not.toHaveBeenCalled()
  })

  it('is silent while the counters are steady', async () => {
    stats.get('speaker')!.compressed = 5
    await tick(1000)
    expect(warn).toHaveBeenCalledTimes(1)
    for (let s = 0; s < 30; s++) await tick(1000)
    expect(warn).toHaveBeenCalledTimes(1)
  })

  it('warns for decode errors and overflow merging as well', async () => {
    await tick(1000)
    stats.get('speaker')!.decodeErrors = 2
    await tick(1000)
    expect(warn).toHaveBeenCalledTimes(1)
    expect(warn.mock.calls[0]![0]).toContain('decodeErrors=+2')

    await tick(10_000)
    stats.get('speaker')!.overflowCompressed = 1
    await tick(1000)
    expect(warn).toHaveBeenCalledTimes(2)
    expect(warn.mock.calls[1]![0]).toContain('overflowCompressed=+1')
  })

  it('logs at most one line per 10 s per peer and folds the skipped increases in', async () => {
    await tick(1000)
    const s = stats.get('speaker')!
    for (let sec = 1; sec <= 25; sec++) {
      s.compressed += 1
      await tick(1000)
    }
    // Increases every second for 25 s: lines at +1 s, +11 s and +21 s only.
    expect(warn).toHaveBeenCalledTimes(3)
    expect(warn.mock.calls[0]![0]).toContain('compressed=+1 ')
    expect(warn.mock.calls[1]![0]).toContain('compressed=+10 ')
    expect(warn.mock.calls[2]![0]).toContain('compressed=+10 ')
  })

  it('never warns when the graph has no mixStats', async () => {
    const graph = createStatsGraph(stats)
    delete graph.mixStats
    const quietWarn = vi.fn<(message: string) => void>()
    const quiet = new ClientAudioMixer({ graph, warn: quietWarn })
    quiet.registerPeer('speaker')
    quiet.registerPeer('listener')
    quiet.startMixPump('listener', out)
    stats.get('speaker')!.compressed = 9
    vi.advanceTimersByTime(2000)
    await quiet.pumpMixFrame('listener', out)
    quiet.stopMixPump('listener')
    expect(quietWarn).not.toHaveBeenCalled()
  })
})
