import { describe, expect, it } from 'vitest'

import { createLiveInboundReader, type StereoEnergyReader } from './mix-energy-helpers.js'

type QueuedFrame = Uint8Array | null | undefined

function createControllableFakeTrack(): {
  track: StereoEnergyReader
  deliver(frame: QueuedFrame): void
  waitForReadCall(callNumber: number): Promise<void>
  readonly readSampleCalls: number
} {
  let pendingReadResolve: ((frame: QueuedFrame) => void) | null = null
  let readSampleCalls = 0

  const track: StereoEnergyReader = {
    readSample() {
      readSampleCalls++
      return new Promise((resolve) => {
        pendingReadResolve = resolve
      })
    },
  }

  return {
    track,
    deliver(frame: QueuedFrame) {
      if (!pendingReadResolve) {
        throw new Error('fake track has no pending readSample()')
      }
      const resolve = pendingReadResolve
      pendingReadResolve = null
      resolve(frame)
    },
    async waitForReadCall(callNumber: number) {
      while (readSampleCalls < callNumber) {
        await Promise.resolve()
      }
    },
    get readSampleCalls() {
      return readSampleCalls
    },
  }
}

describe('createLiveInboundReader', () => {
  it('resolves readSample with the next frame arriving after the call, not one that arrived earlier', async () => {
    const fake = createControllableFakeTrack()
    const reader = createLiveInboundReader(fake.track)

    await fake.waitForReadCall(1)
    fake.deliver(new Uint8Array([1]))
    await fake.waitForReadCall(2)
    fake.deliver(new Uint8Array([2]))
    await Promise.resolve()
    expect(reader.framesRead).toBe(2)

    const readPromise = reader.readSample()
    await fake.waitForReadCall(3)
    fake.deliver(new Uint8Array([3, 4, 5, 6]))
    expect(await readPromise).toEqual(new Uint8Array([3, 4, 5, 6]))
    expect(reader.framesRead).toBe(3)
    reader.stop()
  })

  it('drops frames with no awaiter but still increments framesRead', async () => {
    const fake = createControllableFakeTrack()
    const reader = createLiveInboundReader(fake.track)

    await fake.waitForReadCall(1)
    fake.deliver(new Uint8Array([1]))
    await fake.waitForReadCall(2)
    fake.deliver(new Uint8Array([2]))
    await Promise.resolve()
    expect(reader.framesRead).toBe(2)

    const readPromise = reader.readSample()
    await fake.waitForReadCall(3)
    fake.deliver(new Uint8Array([3]))
    expect(await readPromise).toEqual(new Uint8Array([3]))
    reader.stop()
  })

  it('gives concurrent awaiters the same frame', async () => {
    const fake = createControllableFakeTrack()
    const reader = createLiveInboundReader(fake.track)

    await fake.waitForReadCall(1)

    const frame = new Uint8Array([42, 43, 44, 45])
    const first = reader.readSample()
    const second = reader.readSample()
    fake.deliver(frame)

    expect(await first).toBe(frame)
    expect(await second).toBe(frame)
    reader.stop()
  })

  it('stop resolves pending awaiters with null and the loop stops calling the fake', async () => {
    const fake = createControllableFakeTrack()
    const reader = createLiveInboundReader(fake.track)

    await fake.waitForReadCall(1)

    const pending = reader.readSample()
    reader.stop()
    expect(await pending).toBeNull()
    expect(await reader.readSample()).toBeNull()

    const callsAfterStop = fake.readSampleCalls
    fake.deliver(new Uint8Array([99]))
    await Promise.resolve()
    await Promise.resolve()
    expect(fake.readSampleCalls).toBe(callsAfterStop)
    reader.stop()
  })
})
