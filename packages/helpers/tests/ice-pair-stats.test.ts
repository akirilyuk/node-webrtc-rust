import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  ICE_PAIR_STATS_TIMEOUT_MS,
  formatIcePairLine,
  readSelectedIcePair,
} from '../src/ice-pair-stats.js'
import type { StatsPeerConnection } from '../src/inbound-audio-stats.js'

type Stat = Record<string, unknown>

function fakePc(
  entries: [string, Stat][],
  connectionState = 'connected',
): StatsPeerConnection & { getStats: ReturnType<typeof vi.fn> } {
  return { connectionState, getStats: vi.fn(async () => new Map(entries)) }
}

const NOW = 1_700_000_100_000

const local: [string, Stat] = [
  'cand-l',
  {
    type: 'local-candidate',
    id: 'cand-l',
    candidateType: 'relay',
    networkType: 'udp4',
    port: 50905,
    relayProtocol: 'udp',
    ip: '203.0.113.7',
  },
]
const remote: [string, Stat] = [
  'cand-r',
  { type: 'remote-candidate', id: 'cand-r', candidateType: 'srflx', ip: '198.51.100.9' },
]

function pair(extra: Stat = {}): Stat {
  return {
    type: 'candidate-pair',
    localCandidateId: 'cand-l',
    remoteCandidateId: 'cand-r',
    state: 'succeeded',
    nominated: true,
    packetsSent: 10,
    packetsReceived: 20,
    requestsSent: 3,
    responsesReceived: 2,
    lastPacketReceivedTimestamp: NOW / 1000 - 1.5,
    lastPacketSentTimestamp: NOW / 1000 - 0.5,
    currentRoundTripTime: 0.0234,
    ...extra,
  }
}

describe('readSelectedIcePair', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('picks the nominated succeeded pair and resolves types, protocol, port and counters', async () => {
    const pc = fakePc([
      ['p-other', pair({ nominated: false, packetsSent: 999 })],
      ['p-sel', pair()],
      local,
      remote,
    ])
    const result = await readSelectedIcePair(pc, { nowMs: NOW })
    expect(result).toEqual({
      selected: true,
      localType: 'relay',
      remoteType: 'srflx',
      protocol: 'udp4',
      relayProtocol: 'udp',
      localPort: 50905,
      state: 'succeeded',
      nominated: true,
      packetsSent: 10,
      packetsReceived: 20,
      requestsSent: 3,
      responsesReceived: 2,
      lastRxAgeMs: 1500,
      lastTxAgeMs: 500,
      rttMs: 23,
    })
  })

  it('prefers the pair with the larger lastPacketReceivedTimestamp on a tie', async () => {
    const pc = fakePc([
      ['p1', pair({ packetsSent: 1, lastPacketReceivedTimestamp: NOW / 1000 - 10 })],
      ['p2', pair({ packetsSent: 2, lastPacketReceivedTimestamp: NOW / 1000 - 1 })],
      local,
      remote,
    ])
    const result = await readSelectedIcePair(pc, { nowMs: NOW })
    expect(result).toMatchObject({ selected: true, packetsSent: 2 })
  })

  it('reports counts by state when no pair is selected', async () => {
    const pc = fakePc([
      ['p1', pair({ nominated: false, state: 'in-progress' })],
      ['p2', pair({ state: 'failed' })],
      local,
      remote,
    ])
    const result = await readSelectedIcePair(pc, { nowMs: NOW })
    expect(result).toEqual({
      selected: false,
      pairs: 2,
      states: { 'in-progress': 1, failed: 1 },
    })
    expect(formatIcePairLine(result!)).toBe('ice_pair none pairs=2 states=failed:1,in-progress:1')
  })

  it('uses ? for missing candidate rows', async () => {
    const pc = fakePc([['p1', pair()]])
    const result = await readSelectedIcePair(pc, { nowMs: NOW })
    expect(result).toMatchObject({
      selected: true,
      localType: '?',
      remoteType: '?',
      protocol: '?',
    })
    expect(result).not.toHaveProperty('relayProtocol')
    expect(result).not.toHaveProperty('localPort')
  })

  it('returns null for a missing or closed peer connection', async () => {
    expect(await readSelectedIcePair(undefined)).toBeNull()
    const pc = fakePc([['p1', pair()]], 'closed')
    expect(await readSelectedIcePair(pc)).toBeNull()
    expect(pc.getStats).not.toHaveBeenCalled()
  })

  it('returns null when getStats rejects', async () => {
    const pc: StatsPeerConnection = {
      connectionState: 'connected',
      getStats: vi.fn(async () => {
        throw new Error('boom')
      }),
    }
    expect(await readSelectedIcePair(pc)).toBeNull()
  })

  it('returns null after timeoutMs when getStats never settles', async () => {
    vi.useFakeTimers()
    const pc: StatsPeerConnection = {
      connectionState: 'connected',
      getStats: vi.fn(() => new Promise(() => {})),
    }
    const pending = readSelectedIcePair(pc, { timeoutMs: ICE_PAIR_STATS_TIMEOUT_MS })
    await vi.advanceTimersByTimeAsync(ICE_PAIR_STATS_TIMEOUT_MS)
    await expect(pending).resolves.toBeNull()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('never puts a candidate address in the formatted line', async () => {
    const pc = fakePc([['p1', pair()], local, remote])
    const result = await readSelectedIcePair(pc, { nowMs: NOW })
    const line = formatIcePairLine(result!)
    expect(line).not.toContain('203.0.113.7')
    expect(line).not.toContain('198.51.100.9')
    expect(line).toBe(
      'ice_pair local=relay remote=srflx protocol=udp4 relay_protocol=udp local_port=50905 ' +
        'state=succeeded nominated=true pkts_tx=10 pkts_rx=20 stun_req_tx=3 stun_resp_rx=2 ' +
        'last_rx_ms=1500 last_tx_ms=500 rtt_ms=23',
    )
  })

  it('computes ages from nowMs and omits zero or non-finite timestamps', async () => {
    const pc = fakePc([
      [
        'p1',
        pair({
          lastPacketReceivedTimestamp: NOW / 1000 - 4,
          lastPacketSentTimestamp: 0,
          currentRoundTripTime: 0,
        }),
      ],
      local,
      remote,
    ])
    const result = await readSelectedIcePair(pc, { nowMs: NOW })
    expect(result).toMatchObject({ lastRxAgeMs: 4000 })
    expect(result).not.toHaveProperty('lastTxAgeMs')
    expect(result).not.toHaveProperty('rttMs')
  })
})
