import { afterEach, describe, expect, it } from 'vitest'

import {
  VoiceSessionBudget,
  VoiceSessionBudgetFullError,
  resetProcessVoiceSessionBudget,
  resolveMaxVoiceSessionsFromEnv,
} from '../src/voice-session-budget.js'

describe('VoiceSessionBudget', () => {
  afterEach(() => {
    resetProcessVoiceSessionBudget()
  })

  it('allows unlimited sessions when max is 0', () => {
    const budget = new VoiceSessionBudget(0)
    expect(budget.tryAcquire('client-1')).toBeTypeOf('string')
    expect(budget.tryAcquire('client-2')).toBeTypeOf('string')
    expect(budget.snapshot().max).toBe(0)
    expect(budget.snapshot().available).toBe(Number.POSITIVE_INFINITY)
  })

  it('enforces max concurrent sessions via opaque leases', () => {
    const budget = new VoiceSessionBudget(2)
    const a = budget.tryAcquire('client-1')
    const b = budget.tryAcquire('client-2')
    expect(a).toBeTypeOf('string')
    expect(b).toBeTypeOf('string')
    expect(a).not.toBe(b)
    expect(budget.tryAcquire('client-3')).toBeNull()
    expect(budget.snapshot()).toMatchObject({
      active: 2,
      max: 2,
      available: 0,
      rejectedTotal: 1,
    })
  })

  it('releases only with the opaque lease token', () => {
    const budget = new VoiceSessionBudget(1)
    const lease = budget.tryAcquire('client-1')
    expect(lease).toBeTypeOf('string')
    expect(budget.tryAcquire('client-2')).toBeNull()
    budget.release('not-a-real-lease')
    expect(budget.snapshot().active).toBe(1)
    budget.release(lease!)
    expect(budget.snapshot().active).toBe(0)
    expect(budget.tryAcquire('client-2')).toBeTypeOf('string')
  })

  it('same peerId across hosts each consume capacity (opaque leases)', () => {
    const budget = new VoiceSessionBudget(2)
    const hostA = budget.tryAcquire('client-same')
    const hostB = budget.tryAcquire('client-same')
    expect(hostA).toBeTypeOf('string')
    expect(hostB).toBeTypeOf('string')
    expect(hostA).not.toBe(hostB)
    expect(budget.snapshot().active).toBe(2)
    expect(budget.tryAcquire('client-other')).toBeNull()
    budget.release(hostA!)
    expect(budget.snapshot().active).toBe(1)
    expect(budget.hasLease(hostB!)).toBe(true)
  })

  it('acquire throws VoiceSessionBudgetFullError', () => {
    const budget = new VoiceSessionBudget(1)
    budget.acquire('client-1')
    expect(() => budget.acquire('client-2')).toThrow(VoiceSessionBudgetFullError)
  })

  it('parses VOICE_MAX_CONCURRENT_SESSIONS from env', () => {
    expect(resolveMaxVoiceSessionsFromEnv({ VOICE_MAX_CONCURRENT_SESSIONS: '8' })).toBe(8)
    expect(resolveMaxVoiceSessionsFromEnv({})).toBe(0)
    expect(resolveMaxVoiceSessionsFromEnv({ VOICE_MAX_CONCURRENT_SESSIONS: 'bad' })).toBe(0)
  })
})
