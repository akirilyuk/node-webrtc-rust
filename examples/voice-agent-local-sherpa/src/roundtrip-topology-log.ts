/**
 * Sherpa roundtrip topology + WebRTC role logging.
 *
 * Makes it obvious **which side** is doing what in loopback E2E:
 *   - **signaling** — SignalingServer / ws negotiation
 *   - **agent-pc**  — impolite peer (listener VoiceAgent attach target)
 *   - **user-pc**   — polite peer (user simulator / second agent)
 *
 * Opt out: SHERPA_ROUNDTRIP_TOPOLOGY_LOG=0
 */

import type { RTCPeerConnection } from '@node-webrtc-rust/sdk'

export type RoundtripTopologyRole = 'signaling' | 'agent-pc' | 'user-pc' | 'listener' | 'user-sim'

export interface RoundtripAttachSummary {
  role: RoundtripTopologyRole
  /** Human label, e.g. "listener VoiceAgent" or "user TTS simulator" */
  label: string
  inboundTrack: string
  outboundTrack: string
}

function topologyEnabled(): boolean {
  return process.env.SHERPA_ROUNDTRIP_TOPOLOGY_LOG !== '0'
}

function log(role: RoundtripTopologyRole, message: string): void {
  if (!topologyEnabled()) return
  console.error(`[topology] [${role}] ${message}`)
}

/** Banner at script start — script name + env knobs. */
export function logRoundtripScriptBanner(params: {
  script: string
  pipeline: string
  extra?: string[]
}): void {
  if (!topologyEnabled()) return
  console.error('')
  console.error(`[topology] === ${params.script} ===`)
  console.error(`[topology] pipeline: ${params.pipeline}`)
  for (const line of params.extra ?? []) {
    console.error(`[topology] ${line}`)
  }
  console.error('[topology] roles: signaling server | agent-pc (listener) | user-pc (simulator)')
  console.error(
    '[topology] stderr: [topology] attach/ICE | [speech] events | [voice-debug] Rust STT/VAD',
  )
  console.error('')
}

export function logSignalingReady(params: { port: number; room?: string }): void {
  log(
    'signaling',
    `SignalingServer listening ws://127.0.0.1:${params.port} room=${params.room ?? 'voice-demo'}`,
  )
}

export function logVoiceAgentAttach(summary: RoundtripAttachSummary): void {
  log(
    summary.role,
    `${summary.label}: inbound=${summary.inboundTrack} outbound=${summary.outboundTrack}`,
  )
}

export function logE2ePhase(params: { phase: string; detail?: string }): void {
  if (!topologyEnabled()) return
  const detail = params.detail != null ? ` — ${params.detail}` : ''
  console.error(`[e2e-phase] ${params.phase}${detail}`)
}

/** Log ICE / connection state transitions for both loopback peers. */
export function attachRoundtripConnectionLogs(params: {
  agentPc: RTCPeerConnection
  userPc: RTCPeerConnection
  agentPeerId?: string
  userPeerId?: string
}): void {
  if (!topologyEnabled()) return

  const agentId = params.agentPeerId ?? 'agent'
  const userId = params.userPeerId ?? 'user'

  const watch = (pc: RTCPeerConnection, role: 'agent-pc' | 'user-pc', peerId: string) => {
    let lastConn = pc.connectionState
    let lastIce = pc.iceConnectionState
    log(role, `${peerId} initial connection=${lastConn} ice=${lastIce}`)

    pc.onconnectionstatechange = () => {
      const next = pc.connectionState
      if (next !== lastConn) {
        log(role, `${peerId} connection ${lastConn} → ${next}`)
        lastConn = next
      }
    }
    pc.oniceconnectionstatechange = () => {
      const next = pc.iceConnectionState
      if (next !== lastIce) {
        log(role, `${peerId} ice ${lastIce} → ${next}`)
        lastIce = next
      }
    }
  }

  watch(params.agentPc, 'agent-pc', agentId)
  watch(params.userPc, 'user-pc', userId)
}
