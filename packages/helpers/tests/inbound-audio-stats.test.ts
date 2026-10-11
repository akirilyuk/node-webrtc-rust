import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  INBOUND_AUDIO_STATS_TIMEOUT_MS,
  readInboundAudioStats,
  type StatsPeerConnection,
} from '../src/inbound-audio-stats.js'

type Stat = Record<string, unknown>

function fakePc(
  stats: Stat[],
  connectionState = 'connected',
): StatsPeerConnection & { getStats: ReturnType<typeof vi.fn> } {
  return {
    connectionState,
    getStats: vi.fn(async () => new Map(stats.map((stat, index) => [`stat-${index}`, stat]))),
  }
}

describe('readInboundAudioStats', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('sums every inbound audio entry', async () => {
    const pc = fakePc([
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: 120, bytesReceived: 9600 },
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: 30, bytesReceived: 2400 },
    ])
    await expect(readInboundAudioStats(pc)).resolves.toEqual({
      packetsReceived: 150,
      bytesReceived: 12000,
    })
  })

  it('ignores video and outbound entries', async () => {
    const pc = fakePc([
      { type: 'inbound-rtp', kind: 'video', packetsReceived: 999, bytesReceived: 999 },
      { type: 'outbound-rtp', kind: 'audio', packetsSent: 500, bytesSent: 500 },
      { type: 'outbound-rtp', kind: 'audio', packetsReceived: 777, bytesReceived: 777 },
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: 5, bytesReceived: 400 },
    ])
    await expect(readInboundAudioStats(pc)).resolves.toEqual({
      packetsReceived: 5,
      bytesReceived: 400,
    })
  })

  it('treats an entry with non-number counters as absent', async () => {
    const pc = fakePc([
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: '7', bytesReceived: 100 },
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: 7 },
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: 3, bytesReceived: 240 },
    ])
    await expect(readInboundAudioStats(pc)).resolves.toEqual({
      packetsReceived: 3,
      bytesReceived: 240,
    })
  })

  it('returns null when there is no inbound audio entry', async () => {
    const pc = fakePc([
      { type: 'inbound-rtp', kind: 'video', packetsReceived: 1, bytesReceived: 1 },
      { type: 'transport' },
    ])
    await expect(readInboundAudioStats(pc)).resolves.toBeNull()
  })

  it('returns null when the only audio entry has non-number counters', async () => {
    const pc = fakePc([
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: null, bytesReceived: 'x' },
    ])
    await expect(readInboundAudioStats(pc)).resolves.toBeNull()
  })

  it('returns null for an absent peer connection', async () => {
    await expect(readInboundAudioStats(undefined)).resolves.toBeNull()
  })

  it('returns null for a closed peer connection without calling getStats', async () => {
    const pc = fakePc(
      [{ type: 'inbound-rtp', kind: 'audio', packetsReceived: 1, bytesReceived: 1 }],
      'closed',
    )
    await expect(readInboundAudioStats(pc)).resolves.toBeNull()
    expect(pc.getStats).not.toHaveBeenCalled()
  })

  it('returns null when getStats rejects', async () => {
    const pc: StatsPeerConnection = {
      connectionState: 'connected',
      getStats: () => Promise.reject(new Error('native stats failed')),
    }
    await expect(readInboundAudioStats(pc)).resolves.toBeNull()
  })

  it('returns null when getStats throws synchronously', async () => {
    const pc: StatsPeerConnection = {
      connectionState: 'connected',
      getStats: () => {
        throw new Error('boom')
      },
    }
    await expect(readInboundAudioStats(pc)).resolves.toBeNull()
  })

  it('returns null after the default timeout when getStats hangs', async () => {
    vi.useFakeTimers()
    const pc: StatsPeerConnection = {
      connectionState: 'connected',
      getStats: () => new Promise(() => undefined),
    }
    let settled = false
    const result = readInboundAudioStats(pc).then((value) => {
      settled = true
      return value
    })
    await vi.advanceTimersByTimeAsync(INBOUND_AUDIO_STATS_TIMEOUT_MS - 1)
    expect(settled).toBe(false)
    await vi.advanceTimersByTimeAsync(1)
    await expect(result).resolves.toBeNull()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('honours a custom timeoutMs', async () => {
    vi.useFakeTimers()
    const pc: StatsPeerConnection = {
      connectionState: 'connected',
      getStats: () => new Promise(() => undefined),
    }
    const result = readInboundAudioStats(pc, { timeoutMs: 50 })
    await vi.advanceTimersByTimeAsync(50)
    await expect(result).resolves.toBeNull()
  })

  it('clears the timeout once stats arrive', async () => {
    vi.useFakeTimers()
    const pc = fakePc([
      { type: 'inbound-rtp', kind: 'audio', packetsReceived: 1, bytesReceived: 80 },
    ])
    await expect(readInboundAudioStats(pc)).resolves.toEqual({
      packetsReceived: 1,
      bytesReceived: 80,
    })
    expect(vi.getTimerCount()).toBe(0)
  })
})
