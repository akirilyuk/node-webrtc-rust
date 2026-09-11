/**
 * Shared helpers for clip-playback-smoke and voice-data-mix-smoke SessionPod parity tests.
 * Mirrors e2e probe timing (RMS before play, no waitForClipPlaying) and runner per-session playAudio.
 */

import type { RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import type { AudioPlaySource } from '../src/clip-playback.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { accumulateInboundStereoRms } from './mix-energy-helpers.js'

export const SMOKE_PEER_IDS = ['client-mix-1', 'client-mix-2', 'client-mix-3'] as const

export type SmokeSessionBinding = {
  peerId: string
  host: VoiceAgentSessionHost
}

export const CLIP_RMS_PROBE_MS = 1800

/**
 * Runner `audio-play-control`: one `playAudio({ peerIds: [peerId] })` per targeted session host.
 * G: `peerIds: [listenerPeer]` → listener host only.
 * H: empty `peerIds` → every registered session host, each with that session's peer id.
 */
export async function runnerStylePlayAudio(
  bindings: readonly SmokeSessionBinding[],
  options: { peerIds?: readonly string[] },
  source: AudioPlaySource,
): Promise<string[]> {
  const requested = options.peerIds?.filter(Boolean) ?? []
  const targets =
    requested.length > 0
      ? bindings.filter((binding) => requested.includes(binding.peerId))
      : [...bindings]

  if (targets.length === 0) {
    throw new Error('runnerStylePlayAudio: no play targets resolved')
  }

  const playIds: string[] = []
  for (const { host, peerId } of targets) {
    const { playId } = await host.playAudio({ source, peerIds: [peerId] })
    playIds.push(playId)
  }
  return playIds
}

/** e2e `sendPlayAndProbeInboundEnergy`: start RMS on tracks, then fire play (overlap decode). */
export async function probeClipPlayInboundEnergy(
  bindings: readonly SmokeSessionBinding[],
  playOptions: { peerIds?: readonly string[] },
  tracks: RemoteAudioTrack[],
  source: AudioPlaySource,
  probeMs = CLIP_RMS_PROBE_MS,
): Promise<Array<{ left: number; right: number }>> {
  const rmsFutures = tracks.map((track) => accumulateInboundStereoRms(track, probeMs))
  await runnerStylePlayAudio(bindings, playOptions, source)
  return Promise.all(rmsFutures)
}
