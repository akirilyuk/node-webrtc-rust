import { describe, expect, it } from 'vitest'

import { SessionPodCapacityFullError } from '../src/session-pod.js'

describe('SessionPodCapacityFullError', () => {
  it('reports active and max slot counts', () => {
    const err = new SessionPodCapacityFullError(20, 20)
    expect(err.name).toBe('SessionPodCapacityFullError')
    expect(err.activeSlots).toBe(20)
    expect(err.maxSlots).toBe(20)
    expect(err.message).toBe('session pod capacity full (20/20)')
  })
})
