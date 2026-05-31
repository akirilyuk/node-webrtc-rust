/**
 * Shared helpers for node-webrtc-rust **examples** (not part of the published SDK).
 *
 * Copy patterns from here into your app, or import relatively from example `src/`:
 *   import { DEMO_ICE_SERVERS, waitForConnection } from '../../shared/webrtc-demo-helpers.js'
 *
 * PCM framing conventions live in {@link ./pcm-streaming.ts}.
 */

import type { RTCDataChannel, RTCPeerConnection } from '@node-webrtc-rust/sdk'

/** Public STUN — enough for local demos; production apps should add TURN. */
export const DEMO_ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

export function logSection(title: string): void {
  console.log(`\n=== ${title} ===`)
}

/**
 * Logs W3C connection lifecycle events — parity APIs added in v0.2:
 * `onicegatheringstatechange`, `onsignalingstatechange`, etc.
 */
export function attachPeerStateLoggers(pc: RTCPeerConnection, label: string): void {
  pc.onconnectionstatechange = () => {
    console.log(`[${label}] connectionState=${pc.connectionState}`)
  }
  pc.oniceconnectionstatechange = () => {
    console.log(`[${label}] iceConnectionState=${pc.iceConnectionState}`)
  }
  pc.onicegatheringstatechange = () => {
    console.log(`[${label}] iceGatheringState=${pc.iceGatheringState}`)
  }
  pc.onsignalingstatechange = () => {
    console.log(`[${label}] signalingState=${pc.signalingState}`)
  }
}

export async function waitForConnection(pc: RTCPeerConnection, timeoutMs = 20_000): Promise<void> {
  if (pc.connectionState === 'connected') return

  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out (connectionState=${pc.connectionState})`)),
      timeoutMs,
    )
    const check = () => {
      if (pc.connectionState === 'connected') {
        clearTimeout(timer)
        resolve()
      } else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {
        clearTimeout(timer)
        reject(new Error(`connection ${pc.connectionState}`))
      }
    }
    pc.onconnectionstatechange = check
    check()
  })
}

export async function waitForDataChannelOpen(
  channel: RTCDataChannel,
  timeoutMs = 15_000,
): Promise<void> {
  if (channel.readyState === 'open') return

  await new Promise<void>((resolve, reject) => {
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

/** Summarize {@link RTCPeerConnection.getStats} for demo logging. */
export async function logConnectionStats(pc: RTCPeerConnection, label: string): Promise<void> {
  const stats = await pc.getStats()
  let candidatePairs = 0
  let inboundAudio = 0
  let outboundAudio = 0

  stats.forEach((report: Record<string, unknown>) => {
    if (report.type === 'candidate-pair') candidatePairs++
    if (report.type === 'inbound-rtp' && report.kind === 'audio') inboundAudio++
    if (report.type === 'outbound-rtp' && report.kind === 'audio') outboundAudio++
  })

  console.log(
    `[${label}] getStats(): ${stats.size} entries` +
      ` (candidate-pair=${candidatePairs}, inbound-audio=${inboundAudio}, outbound-audio=${outboundAudio})`,
  )
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** Wait until {@link RTCPeerConnection.iceGatheringState} reaches `complete`. */
export async function waitForIceGatheringComplete(
  pc: RTCPeerConnection,
  timeoutMs = 15_000,
): Promise<void> {
  if (pc.iceGatheringState === 'complete') return

  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`ICE gathering timed out (state=${pc.iceGatheringState})`)),
      timeoutMs,
    )
    const check = () => {
      if (pc.iceGatheringState === 'complete') {
        clearTimeout(timer)
        resolve()
      }
    }
    pc.onicegatheringstatechange = check
    check()
  })
}
