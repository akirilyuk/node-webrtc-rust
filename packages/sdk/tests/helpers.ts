import type { RTCDataChannel, RTCPeerConnection } from '../src'
import type { MessageEvent } from '../src/types'

/**
 * ICE configuration for loopback tests (both peers in this process).
 *
 * Deliberately has no STUN/TURN servers: host candidates are enough for loopback, and a
 * STUN url makes `gatheringComplete()` wait for DNS resolution of the STUN host. webrtc-ice
 * resolves it with an unbounded `lookup_host`, so a stalled resolver on a CI runner holds
 * gathering (once per peer) past the 60 s test timeout. Tests that need a relay or reflexive
 * candidate (`turn.test.ts`) configure their own servers.
 */
export const defaultIceConfig = {
  iceServers: [],
}

export function waitForOpen(channel: RTCDataChannel, timeoutMs = 15_000): Promise<void> {
  if (channel.readyState === 'open') {
    return Promise.resolve()
  }

  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error('timed out waiting for data channel open')),
      timeoutMs,
    )

    channel.onopen = () => {
      clearTimeout(timer)
      resolve()
    }
    channel.onerror = (event) => {
      clearTimeout(timer)
      reject(new Error(event.message ?? 'data channel error'))
    }
  })
}

export function waitForMessage(channel: RTCDataChannel, timeoutMs = 15_000): Promise<MessageEvent> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for message')), timeoutMs)

    channel.onmessage = (event) => {
      clearTimeout(timer)
      resolve(event)
    }
    channel.onerror = (event) => {
      clearTimeout(timer)
      reject(new Error(event.message ?? 'data channel error'))
    }
  })
}

export function waitForConnection(pc: RTCPeerConnection, timeoutMs = 20_000): Promise<void> {
  if (pc.connectionState === 'connected') {
    return Promise.resolve()
  }

  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out waiting for connection (state=${pc.connectionState})`)),
      timeoutMs,
    )

    const check = () => {
      const state = pc.connectionState
      if (state === 'connected') {
        clearTimeout(timer)
        resolve()
      } else if (state === 'failed' || state === 'closed') {
        clearTimeout(timer)
        reject(new Error(`connection ${state}`))
      }
    }

    pc.onconnectionstatechange = check
    check()
  })
}

export function waitForClosed(pc: RTCPeerConnection, timeoutMs = 5_000): Promise<void> {
  if (pc.connectionState === 'closed') {
    return Promise.resolve()
  }

  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      console.warn(`waitForClosed timed out after ${timeoutMs}ms (state=${pc.connectionState})`)
      resolve()
    }, timeoutMs)

    const check = () => {
      if (pc.connectionState === 'closed') {
        clearTimeout(timer)
        resolve()
      }
    }

    pc.onconnectionstatechange = check
    check()
  })
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
