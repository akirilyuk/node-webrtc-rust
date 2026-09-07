import { describe, expect, it } from 'vitest'

import type { ClipPlayerStatus } from '../src/player/types.js'

describe('player types', () => {
  it('accepts clip status shape', () => {
    const status: ClipPlayerStatus = {
      playId: 'id',
      status: 'buffering',
      positionMs: 0,
      bufferedMs: 0,
    }
    expect(status.status).toBe('buffering')
  })
})
