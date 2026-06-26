import { describe, expect, test, vi } from 'vitest'

import {
  ConnectionError,
  createConnectionError,
  formatConnectionErrorSource,
  getRootConnectionErrorHandler,
  reportConnectionError,
  setRootConnectionErrorHandler,
} from '../src/connection-errors'

describe('connection-errors', () => {
  test('ConnectionError carries source metadata', () => {
    const error = createConnectionError('socket hang up', {
      subsystem: 'signaling',
      room: 'room-a',
      peerId: 'peer-1',
      phase: 'socket',
    })

    expect(ConnectionError.is(error)).toBe(true)
    expect(error.source).toEqual({
      subsystem: 'signaling',
      room: 'room-a',
      peerId: 'peer-1',
      phase: 'socket',
    })
    expect(error.message).toBe('socket hang up')
  })

  test('setRootConnectionErrorHandler receives bubbled errors', () => {
    const handler = vi.fn()
    setRootConnectionErrorHandler(handler)

    const error = createConnectionError('datachannel send failed', {
      subsystem: 'webrtc',
      kind: 'datachannel',
      label: 'voice-control',
    })
    expect(reportConnectionError(error)).toBe(true)
    expect(handler).toHaveBeenCalledWith(error)

    setRootConnectionErrorHandler(undefined)
    expect(getRootConnectionErrorHandler()).toBeUndefined()
    expect(reportConnectionError(error)).toBe(false)
  })

  test('formatConnectionErrorSource summarizes subsystem fields', () => {
    expect(
      formatConnectionErrorSource({
        subsystem: 'session',
        sessionId: 'sess-1',
        code: 'WEBRTC_CONNECTION_FAILED',
      }),
    ).toBe('session code=WEBRTC_CONNECTION_FAILED session=sess-1')
  })
})
