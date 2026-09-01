/**
 * Voice+Data client PCM mixer — tees inbound mic into {@link AudioMixGraph},
 * pans TTS, and sums group mix on each outbound 20 ms frame.
 *
 * Spatial pan math lives in native `MixGraph`; helpers only push/render PCM.
 */

import { AudioMixGraph, type ClientPose, type MixPlacement } from '@node-webrtc-rust/sdk/mix'
import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import { PCM_FULL_FRAME_BYTES } from './pcm.js'

const WRAPPED_OUT = Symbol('clientAudioMixerOutbound')
const WRAPPED_IN = Symbol('clientAudioMixerInbound')

/** PCM port used by {@link ClientAudioMixer} (native graph or test mock). */
export interface ClientMixGraph {
  addInput(participantId: string): void
  removeInput(participantId: string): void
  pushFrame(participantId: string, pcm: Buffer): void
  renderOutput(listenerId: string): Buffer
  panTtsFrame(pcm: Buffer): Buffer
  setPose(participantId: string, pose: ClientPose): void
  setPositionalEnabled(enabled: boolean): void
  setDefaultMixPlacement(placement: MixPlacement): void
  setTtsMixPlacement(placement: MixPlacement): void
  setGroupMembers(groupId: string, members: string[]): void
  moveToGroup(participantId: string, groupId: string): void
  removeFromGroup(participantId: string): void
}

export type ClientAudioMixerOptions = {
  /** Inject a mock graph in unit tests; defaults to a new {@link AudioMixGraph}. */
  graph?: ClientMixGraph
}

/** Saturating sum of two equal-length stereo PCM buffers (20 ms frames). */
export function sumStereoPcm(a: Buffer, b: Buffer): Buffer {
  const len = Math.min(a.length, b.length)
  const out = Buffer.alloc(len)
  for (let i = 0; i + 1 < len; i += 2) {
    const sum = a.readInt16LE(i) + b.readInt16LE(i)
    out.writeInt16LE(Math.max(-32_768, Math.min(32_767, sum)), i)
  }
  return out
}

/**
 * Per-room mix graph wiring for Voice+Data sessions.
 * Register peers on connect; unregister on peer close.
 */
export class ClientAudioMixer {
  private readonly graph: ClientMixGraph
  private readonly registered = new Set<string>()

  constructor(options?: ClientAudioMixerOptions) {
    this.graph = options?.graph ?? new AudioMixGraph()
  }

  /** @internal Test access to the underlying graph. */
  getMixGraph(): ClientMixGraph {
    return this.graph
  }

  registerPeer(peerId: string): void {
    if (this.registered.has(peerId)) return
    this.graph.addInput(peerId)
    this.registered.add(peerId)
  }

  unregisterPeer(peerId: string): void {
    if (!this.registered.has(peerId)) return
    this.graph.removeInput(peerId)
    this.graph.removeFromGroup(peerId)
    this.registered.delete(peerId)
  }

  setGroupMembers(groupId: string, clientIds: string[]): void {
    this.graph.setGroupMembers(groupId, clientIds)
  }

  moveToGroup(clientId: string, groupId: string): void {
    this.graph.moveToGroup(clientId, groupId)
  }

  removeFromGroup(clientId: string): void {
    this.graph.removeFromGroup(clientId)
  }

  setClientPose(clientId: string, pose: ClientPose): void {
    this.graph.setPose(clientId, pose)
  }

  setPositionalEnabled(enabled: boolean): void {
    this.graph.setPositionalEnabled(enabled)
  }

  setDefaultMixPlacement(placement: MixPlacement): void {
    this.graph.setDefaultMixPlacement(placement)
  }

  setTtsMixPlacement(placement: MixPlacement): void {
    this.graph.setTtsMixPlacement(placement)
  }

  /**
   * Tee inbound mic PCM into the graph; VoiceAgent still receives the original samples.
   */
  wrapInboundTrack(peerId: string, track: RemoteAudioTrack): RemoteAudioTrack {
    const flagged = track as RemoteAudioTrack & { [WRAPPED_IN]?: boolean }
    if (flagged[WRAPPED_IN]) return track

    const orig = track.readSample.bind(track)
    track.readSample = async () => {
      const pcm = await orig()
      if (pcm.length === PCM_FULL_FRAME_BYTES) {
        this.graph.pushFrame(peerId, pcm)
      }
      return pcm
    }
    flagged[WRAPPED_IN] = true
    return track
  }

  /**
   * On each full TTS frame: pan TTS, render group mix for this listener, sum, then send.
   * Kick/prime frames (non-3840 B) pass through unchanged.
   */
  wrapOutboundTrack(peerId: string, track: LocalAudioTrack): LocalAudioTrack {
    const flagged = track as LocalAudioTrack & { [WRAPPED_OUT]?: boolean }
    if (flagged[WRAPPED_OUT]) return track

    const orig = track.writeSample.bind(track)
    track.writeSample = async (data: Uint8Array | Buffer, durationMs = 20) => {
      const buffer = Buffer.isBuffer(data) ? data : Buffer.from(data)
      if (buffer.length !== PCM_FULL_FRAME_BYTES) {
        return orig(buffer, durationMs)
      }
      const pannedTts = this.graph.panTtsFrame(buffer)
      const mixedClients = this.graph.renderOutput(peerId)
      const outbound = sumStereoPcm(pannedTts, mixedClients)
      return orig(outbound, durationMs)
    }
    flagged[WRAPPED_OUT] = true
    return track
  }
}
