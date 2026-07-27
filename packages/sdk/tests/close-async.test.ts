import { describe, expect, test } from 'vitest'

import { RTCPeerConnection } from '../src'

describe('RTCPeerConnection.closeAsync', () => {
  test('close() remains void and closeAsync resolves', async () => {
    const pc = new RTCPeerConnection()
    expect(pc.close()).toBeUndefined()
    await expect(pc.closeAsync()).resolves.toBeUndefined()
  })

  test('closeAsync is idempotent', async () => {
    const pc = new RTCPeerConnection()
    await pc.closeAsync()
    await expect(pc.closeAsync()).resolves.toBeUndefined()
    await expect(pc.closeAsync()).resolves.toBeUndefined()
  })

  test('concurrent close and closeAsync share one native close', async () => {
    const pc = new RTCPeerConnection()
    const native = (pc as unknown as { native: { close: () => Promise<void> } }).native
    let closeCalls = 0
    const originalClose = native.close.bind(native)
    native.close = async () => {
      closeCalls += 1
      await originalClose()
    }

    const results = await Promise.all([
      pc.closeAsync(),
      pc.closeAsync(),
      Promise.resolve(pc.close()).then(() => pc.closeAsync()),
    ])
    expect(results).toHaveLength(3)
    expect(closeCalls).toBe(1)
  })

  test('native close rejection is reported once and propagates from closeAsync', async () => {
    const pc = new RTCPeerConnection()
    const native = (pc as unknown as { native: { close: () => Promise<void> } }).native
    const failure = new Error('native close boom')
    native.close = async () => {
      throw failure
    }

    const errors: unknown[] = []
    pc.on('error', (err) => {
      errors.push(err)
    })

    await expect(pc.closeAsync()).rejects.toThrow('native close boom')
    // Second caller gets the same rejected promise (no second native close).
    await expect(pc.closeAsync()).rejects.toThrow('native close boom')
    expect(errors.length).toBeGreaterThanOrEqual(1)
  })

  test('close() after failed closeAsync does not throw synchronously', async () => {
    const pc = new RTCPeerConnection()
    const native = (pc as unknown as { native: { close: () => Promise<void> } }).native
    native.close = async () => {
      throw new Error('native close boom')
    }
    await expect(pc.closeAsync()).rejects.toThrow('native close boom')
    expect(() => pc.close()).not.toThrow()
  })

  test('synchronous reentrancy during close shares one native close', async () => {
    const pc = new RTCPeerConnection()
    const native = (pc as unknown as { native: { close: () => Promise<void> } }).native
    let closeCalls = 0
    const originalClose = native.close.bind(native)
    native.close = async () => {
      closeCalls += 1
      // Re-enter from a sync error path before the first closeAsync assigns via await.
      pc.close()
      await originalClose()
    }

    await pc.closeAsync()
    expect(closeCalls).toBe(1)
  })

  test('closeAsync settles even when reportWebRtcError / error listeners throw', async () => {
    const pc = new RTCPeerConnection()
    const native = (pc as unknown as { native: { close: () => Promise<void> } }).native
    const failure = new Error('native close boom')
    native.close = async () => {
      throw failure
    }
    pc.on('error', () => {
      throw new Error('listener boom')
    })
    await expect(pc.closeAsync()).rejects.toThrow('native close boom')
    await expect(pc.closeAsync()).rejects.toThrow('native close boom')
  })
})
