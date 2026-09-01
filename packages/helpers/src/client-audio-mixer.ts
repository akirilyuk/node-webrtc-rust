/**
 * Voice+Data client PCM mixer — tees inbound mic into {@link AudioMixGraph},
 * captures TTS on a sidecar track, and pumps mixed+TTS audio to the PC outbound track.
 *
 * VoiceAgent natively drains TTS to its attach outbound (sidecar) via `setWriteSampleTee`.
 * The peer-connection `LocalAudioTrack` is written only by the 20 ms mix pump.
 */

import { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, type ClientPose, type MixPlacement } from '@node-webrtc-rust/sdk/mix'
import type { RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from './pcm.js'
import { pcmFromWriteSampleTeeArgs } from './session-recorder.js'

const WRAPPED_IN = Symbol('clientAudioMixerInbound')

/** Sidecar track VoiceAgent drains; must support {@link LocalAudioTrack.setWriteSampleTee}. */
export type TtsSidecarTrack = {
  setWriteSampleTee(callback: ((...args: unknown[]) => void) | null): void
}

/** PC outbound track the mix pump writes to. */
export type MixPumpOutboundTrack = {
  writeSample(data: Uint8Array | Buffer, durationMs?: number): Promise<void>
}

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

type PeerMixState = {
  /** Latest full TTS frame from tee; consumed once per pump tick (null → silence). */
  pendingTts: Buffer | null
  pumpInterval?: ReturnType<typeof setInterval>
  sidecar?: TtsSidecarTrack
  pcOutbound?: MixPumpOutboundTrack
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
  private readonly peers = new Map<string, PeerMixState>()
  private readonly silenceFrame = Buffer.alloc(PCM_FULL_FRAME_BYTES)

  constructor(options?: ClientAudioMixerOptions) {
    this.graph = options?.graph ?? new AudioMixGraph()
  }

  /** @internal Test access to the underlying graph. */
  getMixGraph(): ClientMixGraph {
    return this.graph
  }

  private peerState(peerId: string): PeerMixState {
    const state = this.peers.get(peerId)
    if (!state) {
      throw new Error(`ClientAudioMixer: peer ${peerId} is not registered`)
    }
    return state
  }

  registerPeer(peerId: string): void {
    if (this.registered.has(peerId)) return
    this.graph.addInput(peerId)
    this.registered.add(peerId)
    this.peers.set(peerId, { pendingTts: null })
  }

  unregisterPeer(peerId: string): void {
    if (!this.registered.has(peerId)) return
    this.stopMixPump(peerId)
    this.graph.removeInput(peerId)
    this.graph.removeFromGroup(peerId)
    this.registered.delete(peerId)
    this.peers.delete(peerId)
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
   * Wires TTS capture on a sidecar track (not on the peer connection).
   * VoiceAgent.attach uses this track; native drain invokes the tee callback.
   */
  wireTtsSidecar(peerId: string, sidecar: TtsSidecarTrack): TtsSidecarTrack {
    const state = this.peerState(peerId)
    state.sidecar = sidecar
    sidecar.setWriteSampleTee((...args: unknown[]) => {
      const pcm = pcmFromWriteSampleTeeArgs(args)
      if (pcm != null && pcm.length === PCM_FULL_FRAME_BYTES) {
        state.pendingTts = Buffer.from(pcm)
      }
    })
    return sidecar
  }

  /**
   * Creates a native sidecar {@link LocalAudioTrack} for VoiceAgent TTS drain.
   * Not added to the peer connection.
   */
  createTtsSidecar(peerId: string): LocalAudioTrack {
    const state = this.peers.get(peerId)
    if (state?.sidecar && state.sidecar instanceof LocalAudioTrack) {
      return state.sidecar
    }
    const sidecar = new LocalAudioTrack(`agent-tts-${peerId}`, 'voice-agent')
    return this.wireTtsSidecar(peerId, sidecar) as LocalAudioTrack
  }

  /**
   * Starts a 20 ms pump that is the sole writer to the PC outbound track:
   * `sum(panTtsFrame(ttsOrSilence), renderOutput(listener))`.
   */
  startMixPump(peerId: string, pcOutbound: MixPumpOutboundTrack): void {
    const state = this.peerState(peerId)
    if (state.pumpInterval) return

    state.pcOutbound = pcOutbound
    state.pumpInterval = setInterval(() => {
      void this.pumpMixFrame(peerId, pcOutbound).catch(() => undefined)
    }, PCM_FRAME_DURATION_MS)
  }

  stopMixPump(peerId: string): void {
    const state = this.peers.get(peerId)
    if (!state?.pumpInterval) return
    clearInterval(state.pumpInterval)
    state.pumpInterval = undefined
    state.pcOutbound = undefined
  }

  /** @internal One mix tick — exposed for unit tests with fake timers. */
  async pumpMixFrame(peerId: string, pcOutbound?: MixPumpOutboundTrack): Promise<void> {
    const state = this.peers.get(peerId)
    if (!state) return
    const out = pcOutbound ?? state.pcOutbound
    if (!out) return

    const mixed = this.graph.renderOutput(peerId)
    const tts = state.pendingTts ?? this.silenceFrame
    state.pendingTts = null
    const panned = this.graph.panTtsFrame(tts)
    const frame = sumStereoPcm(panned, mixed)
    await out.writeSample(frame, PCM_FRAME_DURATION_MS)
  }
}
