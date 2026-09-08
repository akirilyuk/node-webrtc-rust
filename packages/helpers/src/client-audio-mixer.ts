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

/** Post-mute outbound bursts (~380ms) to replace pre-mute WebRTC playout buffer. */
const MIX_MUTE_FLUSH_FRAMES = 19

/** Sidecar track VoiceAgent drains; must support {@link LocalAudioTrack.setWriteSampleTee}. */
export type TtsSidecarTrack = {
  setWriteSampleTee(callback: ((...args: unknown[]) => void) | null): void
}

/** PC outbound track the mix pump writes to. */
export type MixPumpOutboundTrack = {
  writeSample(data: Uint8Array | Buffer, durationMs?: number): Promise<void>
}

/** Mix-only status snapshot (STT is filled by the owning {@link VoiceAgentSessionHost}). */
export type ClientMixSnapshot = {
  clientId: string
  globallyMuted: boolean
  pose: ClientPose | null
  ttsPose: ClientPose | null
  /** Listener client ids that have muted this target. */
  mutedBy: string[]
  groupId: string | null
}

export type ClientMixStatus = ClientMixSnapshot & {
  sttEnabled: boolean
}

/** PCM port used by {@link ClientAudioMixer} (native graph or test mock). */
export interface ClientMixGraph {
  addInput(participantId: string): void
  removeInput(participantId: string): void
  pushFrame(participantId: string, pcm: Buffer): void
  renderOutput(listenerId: string): Buffer
  panTtsFrame(pcm: Buffer, listenerId: string): Buffer
  setPose(participantId: string, pose: ClientPose): void
  setPositionalEnabled(enabled: boolean): void
  setDefaultMixPlacement(placement: MixPlacement): void
  setTtsMixPlacement(placement: MixPlacement): void
  setTtsPose(participantId: string, pose: ClientPose): void
  clearTtsPose(participantId: string): void
  setGroupMembers(groupId: string, members: string[]): void
  moveToGroup(participantId: string, groupId: string): void
  removeFromGroup(participantId: string): void
  setGlobalMute?(target: string, muted: boolean): void
  isGloballyMuted?(target: string): boolean
  setListenerMute?(listener: string, target: string, muted: boolean): void
  isListenerMuted?(listener: string, target: string): boolean
  pose?(participantId: string): ClientPose | null | undefined
  ttsPose?(participantId: string): ClientPose | null | undefined
  listenerSources?(listener: string): string[] | null
}

type GraphGroupState = {
  memberGroups: Map<string, string>
  groups: Map<string, Set<string>>
  knownClientIds: Set<string>
}

const graphGroupState = new WeakMap<ClientMixGraph, GraphGroupState>()

function getGraphGroupState(graph: ClientMixGraph): GraphGroupState {
  let state = graphGroupState.get(graph)
  if (!state) {
    state = {
      memberGroups: new Map(),
      groups: new Map(),
      knownClientIds: new Set(),
    }
    graphGroupState.set(graph, state)
  }
  return state
}

/** @internal Shared group membership for hosts that inject the same graph. */
export function getSharedMixGroupState(graph: ClientMixGraph): GraphGroupState {
  return getGraphGroupState(graph)
}

function trackClientId(state: GraphGroupState, clientId: string): void {
  state.knownClientIds.add(clientId)
}

function removeClientFromGroupTracking(state: GraphGroupState, clientId: string): void {
  const groupId = state.memberGroups.get(clientId)
  if (groupId) {
    const members = state.groups.get(groupId)
    members?.delete(clientId)
    if (members && members.size === 0) {
      state.groups.delete(groupId)
    }
    state.memberGroups.delete(clientId)
  }
}

function assignClientToGroup(state: GraphGroupState, clientId: string, groupId: string): void {
  removeClientFromGroupTracking(state, clientId)
  trackClientId(state, clientId)
  state.memberGroups.set(clientId, groupId)
  let members = state.groups.get(groupId)
  if (!members) {
    members = new Set()
    state.groups.set(groupId, members)
  }
  members.add(clientId)
}

function syncGroupMembers(state: GraphGroupState, groupId: string, members: string[]): void {
  for (const clientId of members) {
    removeClientFromGroupTracking(state, clientId)
  }
  const existing = state.groups.get(groupId)
  if (existing) {
    for (const clientId of existing) {
      state.memberGroups.delete(clientId)
    }
    state.groups.delete(groupId)
  }
  const memberSet = new Set<string>()
  for (const clientId of members) {
    trackClientId(state, clientId)
    state.memberGroups.set(clientId, groupId)
    memberSet.add(clientId)
  }
  if (memberSet.size > 0) {
    state.groups.set(groupId, memberSet)
  }
}

export function clientsShareMixGroup(
  graph: ClientMixGraph,
  listenerId: string,
  targetId: string,
): boolean {
  const state = getGraphGroupState(graph)
  const listenerGroup = state.memberGroups.get(listenerId)
  const targetGroup = state.memberGroups.get(targetId)
  return listenerGroup != null && listenerGroup === targetGroup
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
  /** Serializes mix pump writes per peer (interval + mute flush share one chain). */
  private readonly pumpWrites = new Map<string, Promise<void>>()

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
    trackClientId(getGraphGroupState(this.graph), peerId)
    this.registered.add(peerId)
    this.peers.set(peerId, { pendingTts: null })
  }

  unregisterPeer(peerId: string): void {
    if (!this.registered.has(peerId)) return
    this.stopMixPump(peerId)
    this.graph.removeInput(peerId)
    this.graph.removeFromGroup(peerId)
    removeClientFromGroupTracking(getGraphGroupState(this.graph), peerId)
    getGraphGroupState(this.graph).knownClientIds.delete(peerId)
    this.registered.delete(peerId)
    this.peers.delete(peerId)
  }

  setGroupMembers(groupId: string, clientIds: string[]): void {
    syncGroupMembers(getGraphGroupState(this.graph), groupId, clientIds)
    this.graph.setGroupMembers(groupId, clientIds)
  }

  moveToGroup(clientId: string, groupId: string): void {
    assignClientToGroup(getGraphGroupState(this.graph), clientId, groupId)
    this.graph.moveToGroup(clientId, groupId)
  }

  removeFromGroup(clientId: string): void {
    removeClientFromGroupTracking(getGraphGroupState(this.graph), clientId)
    this.graph.removeFromGroup(clientId)
  }

  async setGlobalMute(targetId: string, muted: boolean): Promise<void> {
    if (!this.graph.setGlobalMute) {
      throw new Error('Mix graph does not support global mute')
    }
    await this.pauseAllMixPumps()
    this.graph.setGlobalMute(targetId, muted)
    if (muted) {
      this.graph.pushFrame(targetId, Buffer.alloc(PCM_FULL_FRAME_BYTES))
    }
    try {
      if (muted) {
        await this.flushAllListenerOutbounds(targetId)
      }
    } finally {
      this.resumeAllMixPumps()
    }
  }

  /** @internal Burst post-mute mix on every registered listener except the muted source. */
  async flushAllListenerOutbounds(
    excludedSourceId: string,
    frames = MIX_MUTE_FLUSH_FRAMES,
  ): Promise<void> {
    for (const peerId of this.registered) {
      if (peerId === excludedSourceId) continue
      await this.burstOutboundMix(peerId, frames)
    }
  }

  /** @internal Pause every mix pump and drain in-flight writes before graph mute. */
  async pauseAllMixPumps(): Promise<void> {
    await Promise.all([...this.registered].map((peerId) => this.pauseMixPump(peerId)))
  }

  /** @internal Resume mix pumps paused by {@link pauseAllMixPumps}. */
  resumeAllMixPumps(): void {
    for (const peerId of this.registered) {
      this.resumeMixPump(peerId)
    }
  }

  /** @internal Post-mute mix burst via {@link pumpMixFrame} (pumps must already be paused). */
  async burstOutboundMix(peerId: string, frames = MIX_MUTE_FLUSH_FRAMES): Promise<void> {
    for (let i = 0; i < frames; i++) {
      await this.enqueuePumpMixFrame(peerId)
    }
  }

  /** Pause one listener pump, burst post-mute mix, then resume. */
  async flushOutboundMix(peerId: string, frames = MIX_MUTE_FLUSH_FRAMES): Promise<void> {
    await this.pauseMixPump(peerId)
    try {
      await this.burstOutboundMix(peerId, frames)
    } finally {
      this.resumeMixPump(peerId)
    }
  }

  /** @internal Pause one listener mix pump and drain in-flight writes. */
  async pauseMixPump(peerId: string): Promise<void> {
    const state = this.peers.get(peerId)
    if (!state) return
    if (state.pumpInterval) {
      clearInterval(state.pumpInterval)
      state.pumpInterval = undefined
    }
    await (this.pumpWrites.get(peerId) ?? Promise.resolve())
  }

  /** @internal Resume one listener mix pump after {@link pauseMixPump}. */
  resumeMixPump(peerId: string): void {
    const state = this.peers.get(peerId)
    if (!state?.pcOutbound || state.pumpInterval) return
    const out = state.pcOutbound
    state.pumpInterval = setInterval(() => {
      void this.enqueuePumpMixFrame(peerId, out).catch(() => undefined)
    }, PCM_FRAME_DURATION_MS)
  }

  async setListenerMute(listenerId: string, targetId: string, muted: boolean): Promise<void> {
    if (listenerId === targetId) {
      throw new Error(`Cannot listener-mute self (${listenerId})`)
    }
    if (!clientsShareMixGroup(this.graph, listenerId, targetId)) {
      throw new Error(`Listener mute requires ${listenerId} and ${targetId} in the same mix group`)
    }
    if (!this.graph.setListenerMute) {
      throw new Error('Mix graph does not support listener mute')
    }
    this.graph.setListenerMute(listenerId, targetId, muted)
  }

  /** Mix snapshot without STT (host / SessionPod fills {@link ClientMixStatus.sttEnabled}). */
  getMixSnapshot(clientId: string): ClientMixSnapshot {
    const state = getGraphGroupState(this.graph)
    const mutedBy: string[] = []
    for (const listenerId of state.knownClientIds) {
      if (listenerId === clientId) continue
      if (this.graph.isListenerMuted?.(listenerId, clientId)) {
        mutedBy.push(listenerId)
      }
    }
    mutedBy.sort()
    const pose = this.graph.pose?.(clientId) ?? null
    const ttsPose = this.graph.ttsPose?.(clientId) ?? null
    return {
      clientId,
      globallyMuted: this.graph.isGloballyMuted?.(clientId) ?? false,
      pose: pose ?? null,
      ttsPose: ttsPose ?? null,
      mutedBy,
      groupId: state.memberGroups.get(clientId) ?? null,
    }
  }

  listMixSnapshots(): ClientMixSnapshot[] {
    const state = getGraphGroupState(this.graph)
    return [...state.knownClientIds].sort().map((clientId) => this.getMixSnapshot(clientId))
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

  setTtsPose(clientId: string, pose: ClientPose): void {
    this.graph.setTtsPose(clientId, pose)
  }

  clearTtsPose(clientId: string): void {
    this.graph.clearTtsPose(clientId)
  }

  /**
   * Tee inbound mic PCM into the graph; VoiceAgent still receives the original samples.
   */
  wrapInboundTrack(peerId: string, track: RemoteAudioTrack): RemoteAudioTrack {
    const flagged = track as RemoteAudioTrack & { [WRAPPED_IN]?: boolean }
    if (flagged[WRAPPED_IN]) return track

    let leftover = Buffer.alloc(0)
    const orig = track.readSample.bind(track)
    track.readSample = async () => {
      const pcm = await orig()
      if (this.graph.isGloballyMuted?.(peerId)) {
        return pcm
      }
      const combined = leftover.length > 0 ? Buffer.concat([leftover, pcm]) : pcm
      let offset = 0
      while (offset + PCM_FULL_FRAME_BYTES <= combined.length) {
        this.graph.pushFrame(peerId, combined.subarray(offset, offset + PCM_FULL_FRAME_BYTES))
        offset += PCM_FULL_FRAME_BYTES
      }
      leftover = offset < combined.length ? Buffer.from(combined.subarray(offset)) : Buffer.alloc(0)
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
      void this.enqueuePumpMixFrame(peerId, pcOutbound).catch(() => undefined)
    }, PCM_FRAME_DURATION_MS)
  }

  stopMixPump(peerId: string): void {
    const state = this.peers.get(peerId)
    if (!state?.pumpInterval) return
    clearInterval(state.pumpInterval)
    state.pumpInterval = undefined
    state.pcOutbound = undefined
    this.pumpWrites.delete(peerId)
  }

  private enqueuePumpMixFrame(peerId: string, pcOutbound?: MixPumpOutboundTrack): Promise<void> {
    const prev = this.pumpWrites.get(peerId) ?? Promise.resolve()
    const next = prev.catch(() => undefined).then(() => this.pumpMixFrame(peerId, pcOutbound))
    this.pumpWrites.set(peerId, next)
    return next
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
    const panned = this.graph.panTtsFrame(tts, peerId)
    const frame = sumStereoPcm(panned, mixed)
    await out.writeSample(frame, PCM_FRAME_DURATION_MS)
  }
}
