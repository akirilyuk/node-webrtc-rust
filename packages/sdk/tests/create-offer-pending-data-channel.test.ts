import { afterEach, describe, expect, it, vi } from 'vitest'

import { RTCPeerConnection } from '../src'

import { defaultIceConfig } from './helpers'

type NativeLike = {
  createDataChannel(label: string, options?: unknown): Promise<unknown>
}

/** Reaches the private native peer connection to delay or break its `createDataChannel`. */
function nativeOf(pc: RTCPeerConnection): NativeLike {
  return (pc as unknown as { native: NativeLike }).native
}

function delayNativeCreate(pc: RTCPeerConnection, delayMs: number): void {
  const native = nativeOf(pc)
  const original = native.createDataChannel.bind(native)
  native.createDataChannel = (label, options) =>
    new Promise((resolve) => setTimeout(resolve, delayMs)).then(() => original(label, options))
}

describe('createOffer with a pending native createDataChannel', () => {
  const pcs: RTCPeerConnection[] = []

  function newPc(): RTCPeerConnection {
    const pc = new RTCPeerConnection(defaultIceConfig)
    pcs.push(pc)
    return pc
  }

  afterEach(() => {
    vi.useRealTimers()
    for (const pc of pcs.splice(0)) {
      pc.close()
    }
  })

  it('offer includes the data channel when the native create completes 150 ms late', async () => {
    const pc = newPc()
    delayNativeCreate(pc, 150)
    pc.createDataChannel('x')

    const offer = await pc.createOffer()

    expect(offer.sdp).toContain('m=application')
  })

  it('offer without pending channels does not wait', async () => {
    const pc = newPc()

    const offer = await pc.createOffer()

    expect(offer.sdp).not.toContain('m=application')
  })

  it('a native create that never settles fails createOffer after 5000 ms', async () => {
    const pc = newPc()
    nativeOf(pc).createDataChannel = () => new Promise(() => {})
    pc.createDataChannel('x')
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })

    const offer = pc.createOffer()
    const outcome = expect(offer).rejects.toThrow('did not complete within 5000 ms')
    await vi.advanceTimersByTimeAsync(5_000)

    await outcome
  })

  it('a rejected native create does not block the offer', async () => {
    const pc = newPc()
    nativeOf(pc).createDataChannel = () => Promise.reject(new Error('native create failed'))
    const channel = pc.createDataChannel('x')
    const errors: string[] = []
    channel.onerror = (event) => {
      errors.push(event.message ?? '')
    }
    await channel.nativeReady.catch(() => undefined)

    const offer = await pc.createOffer()

    expect(offer.sdp).toContain('v=0')
    expect(errors).toEqual(['native create failed'])
  })
})
