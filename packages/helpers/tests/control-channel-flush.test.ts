import { describe, expect, it, vi } from 'vitest'

import { flushVoiceControlChannel } from '../src/control-channel-flush.js'

describe('flushVoiceControlChannel', () => {
  it('returns true when bufferedAmount is already zero', async () => {
    const channel = {
      readyState: 'open',
      bufferedAmount: 0,
    } as RTCDataChannel

    await expect(flushVoiceControlChannel(channel, 100)).resolves.toBe(true)
  })

  it('returns false when the channel is not open', async () => {
    const channel = {
      readyState: 'closed',
      bufferedAmount: 0,
    } as RTCDataChannel

    await expect(flushVoiceControlChannel(channel, 100)).resolves.toBe(false)
  })

  it('waits until bufferedAmount drains', async () => {
    vi.useFakeTimers()
    let bufferedAmount = 512
    const channel = {
      readyState: 'open',
      get bufferedAmount() {
        return bufferedAmount
      },
    } as RTCDataChannel

    const promise = flushVoiceControlChannel(channel, 100)
    await vi.advanceTimersByTimeAsync(10)
    bufferedAmount = 0
    await vi.advanceTimersByTimeAsync(10)
    await expect(promise).resolves.toBe(true)
    vi.useRealTimers()
  })

  it('returns false when flush times out', async () => {
    vi.useFakeTimers()
    const channel = {
      readyState: 'open',
      bufferedAmount: 1024,
    } as RTCDataChannel

    const promise = flushVoiceControlChannel(channel, 30)
    await vi.advanceTimersByTimeAsync(40)
    await expect(promise).resolves.toBe(false)
    vi.useRealTimers()
  })
})
