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

describe('RTCDataChannel.nativeReady', () => {
  test('is already resolved for a channel built from an existing native object', async () => {
    const channel = new RTCDataChannel(createMockNative())
    await expect(channel.nativeReady).resolves.toBeUndefined()
  })

  test('resolves only after the native create resolves, with the native channel attached', async () => {
    let resolveNative!: (native: NativeDataChannel) => void
    const pending = new Promise<NativeDataChannel>((resolve) => {
      resolveNative = resolve
    })
    const channel = RTCDataChannel.fromNativePromise(pending, 'voice-control')
    const order: string[] = []
    const ready = channel.nativeReady.then(() => {
      order.push('ready')
      expect((channel as unknown as { native: unknown }).native).not.toBeNull()
    })
    await new Promise((resolve) => setImmediate(resolve))
    expect(order).toEqual([])
    order.push('native-resolved')
    resolveNative(createMockNative())
    await ready
    expect(order).toEqual(['native-resolved', 'ready'])
  })

  test('rejects with the native error when the create fails', async () => {
    const channel = RTCDataChannel.fromNativePromise(
      Promise.reject(new Error('create failed')),
      'voice-control',
    )
    await expect(channel.nativeReady).rejects.toThrow('create failed')
  })

  test('an unobserved failed create is not an unhandled rejection', async () => {
    const unhandled = vi.fn()
    process.on('unhandledRejection', unhandled)
    try {
      RTCDataChannel.fromNativePromise(Promise.reject(new Error('create failed')), 'x')
      await new Promise((resolve) => setTimeout(resolve, 20))
      expect(unhandled).not.toHaveBeenCalled()
    } finally {
      process.off('unhandledRejection', unhandled)
    }
  })
})
