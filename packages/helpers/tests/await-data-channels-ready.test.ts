import { afterEach, describe, expect, it, vi } from 'vitest'

import { awaitDataChannelsReady } from '../src/voice-agent-session-host.js'

afterEach(() => {
  vi.useRealTimers()
})

describe('awaitDataChannelsReady', () => {
  it('resolves once every channel is native-ready, ignoring an absent sync channel', async () => {
    await expect(
      awaitDataChannelsReady([{ nativeReady: Promise.resolve() }, undefined], 5_000),
    ).resolves.toBeUndefined()
  })

  it('rejects with the native error', async () => {
    await expect(
      awaitDataChannelsReady([{ nativeReady: Promise.reject(new Error('create failed')) }], 5_000),
    ).rejects.toThrow('create failed')
  })

  it('a nativeReady that never settles rejects after 5000 ms', async () => {
    vi.useFakeTimers()
    const settled = vi.fn()
    const result = awaitDataChannelsReady([{ nativeReady: new Promise<void>(() => {}) }], 5_000)
    result.catch(settled)
    await vi.advanceTimersByTimeAsync(4_999)
    expect(settled).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    await expect(result).rejects.toThrow('data channel create timed out after 5000ms')
  })
})
