/**
 * Models three browser tabs joining one room: each tab is one `client-*` peer.
 * The server should accept at most `max` VoiceAgents when `VOICE_MAX_CONCURRENT_SESSIONS` is set.
 *
 * This test exercises the same {@link VoiceSessionBudget} logic used by
 * {@link VoiceAgentSessionHost} without WebRTC or Sherpa weights.
 */

import { describe, expect, it } from 'vitest'

import { VoiceSessionBudget } from '../../../packages/helpers/src/voice-session-budget.js'

describe('three clients in one room (session budget)', () => {
  it('accepts three peers when unlimited', () => {
    const budget = new VoiceSessionBudget(0)
    const peers = ['client-tab1', 'client-tab2', 'client-tab3']
    const leases = peers.map((peerId) => budget.tryAcquire(peerId))
    for (const lease of leases) {
      expect(lease).toBeTypeOf('string')
    }
    expect(budget.snapshot().active).toBe(3)
    expect(budget.snapshot().rejectedTotal).toBe(0)
  })

  it('rejects the third peer when max is 2 (deployment cap)', () => {
    const budget = new VoiceSessionBudget(2)
    expect(budget.tryAcquire('client-tab1')).toBeTypeOf('string')
    expect(budget.tryAcquire('client-tab2')).toBeTypeOf('string')
    expect(budget.tryAcquire('client-tab3')).toBeNull()

    const snap = budget.snapshot()
    expect(snap.active).toBe(2)
    expect(snap.max).toBe(2)
    expect(snap.available).toBe(0)
    expect(snap.rejectedTotal).toBe(1)
  })

  it('allows a new tab after one disconnects', () => {
    const budget = new VoiceSessionBudget(2)
    const a = budget.tryAcquire('client-tab1')
    const b = budget.tryAcquire('client-tab2')
    expect(a).toBeTypeOf('string')
    expect(b).toBeTypeOf('string')
    expect(budget.tryAcquire('client-tab3')).toBeNull()

    budget.release(a!)
    expect(budget.tryAcquire('client-tab3')).toBeTypeOf('string')
    expect(budget.snapshot().active).toBe(2)
  })
})
