/**
 * Shared helpers for clip-playback-smoke and voice-data-mix-smoke SessionPod parity tests.
 * Clip probes: wait for inbound energy on tracks[0], then collect RMS (play/return can precede PCM).
 * Runner per-session playAudio via runnerStylePlayAudio.
 */

import type { AudioPlaySource } from '../src/clip-playback.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import {
  accumulateInboundStereoRms,
  type StereoEnergyReader,
  waitForInboundStereoEnergy,
} from './mix-energy-helpers.js'

export const SMOKE_PEER_IDS = ['client-mix-1', 'client-mix-2', 'client-mix-3'] as const

export type SmokeSessionBinding = {
  peerId: string
  host: VoiceAgentSessionHost
}

export const CLIP_RMS_PROBE_MS = 1800
export const CLIP_INBOUND_ENERGY_THRESHOLD = 200

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

/** Fire play, wait for inbound energy on tracks[0], then collect stereo RMS on all tracks. */
export async function probeClipPlayInboundEnergy(
  bindings: readonly SmokeSessionBinding[],
  playOptions: { peerIds?: readonly string[] },
  tracks: StereoEnergyReader[],
  source: AudioPlaySource,
  probeMs = CLIP_RMS_PROBE_MS,
): Promise<Array<{ left: number; right: number }>> {
  if (tracks.length === 0) {
    throw new Error('probeClipPlayInboundEnergy: tracks must not be empty')
  }
  await runnerStylePlayAudio(bindings, playOptions, source)
  await waitForInboundStereoEnergy(tracks[0], {
    threshold: CLIP_INBOUND_ENERGY_THRESHOLD,
    timeoutMs: 30_000,
    label: 'clip probe inbound',
  })
  return Promise.all(tracks.map((track) => accumulateInboundStereoRms(track, probeMs)))
}
