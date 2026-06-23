import { describe, expect, test, vi } from 'vitest'

import type { JsRTCDataChannel as NativeDataChannel } from '@node-webrtc-rust/bindings'

import { RTCDataChannel } from '../src/RTCDataChannel'

function createMockNative(overrides: Partial<NativeDataChannel> = {}): NativeDataChannel {
  return {
    label: 'voice-control',
    id: 1,
    readyState: 'open',
    bufferedAmount: async () => 0,
    send: async () => undefined,
    close: async () => undefined,
    setBufferedAmountLowThreshold: () => undefined,
    setOnOpen: () => undefined,
    setOnMessage: () => undefined,
    setOnClose: () => undefined,
    setOnError: () => undefined,
    setOnBufferedAmountLow: () => undefined,
    ...overrides,
  }
}

describe('RTCDataChannel error handling', () => {
  test('send on closed channel is dropped without throwing', () => {
    const channel = new RTCDataChannel(createMockNative(), { label: 'voice-control' })
    channel.readyState = 'closed'

    expect(() => channel.send('{"type":"speech_event"}')).not.toThrow()
  })

  test('async send failure does not crash when no error listeners', async () => {
    const channel = new RTCDataChannel(
      createMockNative({
        send: async () => {
          throw new Error("Invalid state: data channel 'voice-control' is closed")
        },
      }),
      { label: 'voice-control' },
    )

    expect(() => channel.send('hello')).not.toThrow()
    await new Promise((resolve) => setImmediate(resolve))
  })

  test('async send failure invokes onerror when set', async () => {
    const onerror = vi.fn()
    const channel = new RTCDataChannel(
      createMockNative({
        send: async () => {
          throw new Error("Invalid state: data channel 'voice-control' is closed")
        },
      }),
      { label: 'voice-control' },
    )
    channel.onerror = onerror

    channel.send('hello')
    await new Promise((resolve) => setImmediate(resolve))

    expect(onerror).toHaveBeenCalledWith(
      expect.objectContaining({
        type: 'error',
        message: "Invalid state: data channel 'voice-control' is closed",
      }),
    )
  })
})
