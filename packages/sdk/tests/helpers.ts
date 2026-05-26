import type { RTCDataChannel, RTCPeerConnection } from '../src'
import type { MessageEvent } from '../src/types'

const STUN_SERVER = { urls: 'stun:stun.l.google.com:19302' }

export const defaultIceConfig = {
  iceServers: [STUN_SERVER],
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

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
