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
    expect(budget.tryAcquire('client-1')).toBe(true)
    expect(budget.tryAcquire('client-2')).toBe(true)
    expect(budget.snapshot().max).toBe(0)
    expect(budget.snapshot().available).toBe(Number.POSITIVE_INFINITY)
  })

  it('enforces max concurrent sessions', () => {
    const budget = new VoiceSessionBudget(2)
    expect(budget.tryAcquire('client-1')).toBe(true)
    expect(budget.tryAcquire('client-2')).toBe(true)
    expect(budget.tryAcquire('client-3')).toBe(false)
    expect(budget.snapshot()).toMatchObject({
      active: 2,
      max: 2,
      available: 0,
      rejectedTotal: 1,
    })
  })

  it('releases slots on disconnect', () => {
    const budget = new VoiceSessionBudget(1)
    expect(budget.tryAcquire('client-1')).toBe(true)
    expect(budget.tryAcquire('client-2')).toBe(false)
    budget.release('client-1')
    expect(budget.tryAcquire('client-2')).toBe(true)
    expect(budget.snapshot().active).toBe(1)
  })

  it('tryAcquire is idempotent per peerId', () => {
    const budget = new VoiceSessionBudget(1)
    expect(budget.tryAcquire('client-1')).toBe(true)
    expect(budget.tryAcquire('client-1')).toBe(true)
    expect(budget.snapshot().active).toBe(1)
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
