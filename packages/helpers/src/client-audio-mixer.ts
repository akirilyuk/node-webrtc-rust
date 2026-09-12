/**
 * Voice+Data client PCM mixer — tees inbound mic into {@link AudioMixGraph},
 * captures TTS on a sidecar track, and pumps mixed+TTS audio to the PC outbound track.
 *
 * VoiceAgent natively drains TTS to its attach outbound (sidecar) via `setWriteSampleTee`.
 * The peer-connection `LocalAudioTrack` is written only by the wall-clock 20 ms mix pump:
 * one outbound frame per due slot (TTS from the sidecar queue, or silence when idle).
 * During an active TTS stream (sidecar frame within 60 ms or queue was non-empty last tick),
 * empty due slots skip instead of inserting silence. After the active window expires, idle
 * silence resumes so conference mix and browser playout stay alive.
 */

import { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, type ClientPose, type MixPlacement } from '@node-webrtc-rust/sdk/mix'
import type { RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from './pcm.js'
import { pcmFromWriteSampleTeeArgs } from './session-recorder.js'

const WRAPPED_IN = Symbol('clientAudioMixerInbound')

function delayMs(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** Post-mute outbound bursts (~380ms) to replace pre-mute WebRTC playout buffer. */
const MIX_MUTE_FLUSH_FRAMES = 19
/** Post-pose flush (~500ms) — must cover staging ENERGY_PROBE_MS / quiet window after TTS pan flip. */
const MIX_POSE_FLUSH_FRAMES = 25
/** Sidecar TTS FIFO cap (~1 s at 20 ms/frame); oldest dropped on overflow. */
const TTS_QUEUE_MAX_FRAMES = 50
/** Max outbound frames emitted from one interval tick (avoids silence catch-up bursts). */
const PUMP_MAX_FRAMES_PER_TICK = 4
/** Sidecar enqueue within this window keeps the TTS stream "active" (skip silence on empty queue). */
const TTS_ACTIVE_WINDOW_MS = 60
/** After this many consecutive active-window skips (~60 ms), treat TTS as ended and write silence. */
const TTS_MAX_CONSECUTIVE_SKIPS = 3

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
  setSourceMixPlacement?(participantId: string, placement: MixPlacement): void
  clearSourceMixPlacement?(participantId: string): void
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
  setListenerSources?(listener: string, sources: string[]): void
  clearListenerRoutes?(listener: string): void
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
  /** Full sidecar TTS frames queued for the mix pump. */
  ttsQueue: Buffer[]
  /** Frames dropped when {@link TTS_QUEUE_MAX_FRAMES} overflowed (diagnostics). */
  ttsQueueOverflowDrops: number
  /** Partial sidecar tee bytes not yet framed to {@link PCM_FULL_FRAME_BYTES}. */
  sidecarLeftover: Buffer
  pumpInterval?: ReturnType<typeof setInterval>
  sidecar?: TtsSidecarTrack
  pcOutbound?: MixPumpOutboundTrack
  /** Wall-clock pump origin (`performance.now()`). */
  pumpT0: number
  /**
   * Due 20 ms slots consumed since {@link pumpT0}: frames written plus active-window skips.
   * A skipped slot is consumed too — its wall time has passed and must not accumulate as backlog.
   */
  pumpWritten: number
  /** Last sidecar full-frame enqueue time (ms). */
  lastSidecarEnqueueAt: number | null
  /** Whether the TTS queue was non-empty at the previous pump tick. */
  queueWasNonEmptyLastTick: boolean
  /** Consecutive active-window skips without a write. */
  consecutiveTtsSkips: number
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

  /** Registered peer ids on this host plus any peers registered on other mixers sharing this graph. */
  listRegisteredPeers(): string[] {
    const state = getGraphGroupState(this.graph)
    if (state.knownClientIds.size > 0) {
      return [...state.knownClientIds].sort()
    }
    return [...this.registered]
  }

  private peerState(peerId: string): PeerMixState {
    const state = this.peers.get(peerId)
    if (!state) {
      throw new Error(`ClientAudioMixer: peer ${peerId} is not registered`)
    }
    return state
  }

  private createPeerMixState(): PeerMixState {
    return {
      ttsQueue: [],
      ttsQueueOverflowDrops: 0,
      sidecarLeftover: Buffer.alloc(0),
      pumpT0: performance.now(),
      pumpWritten: 0,
      lastSidecarEnqueueAt: null,
      queueWasNonEmptyLastTick: false,
      consecutiveTtsSkips: 0,
    }
  }

  private rebasePumpClock(state: PeerMixState, now = performance.now()): void {
    state.pumpT0 = now - state.pumpWritten * PCM_FRAME_DURATION_MS
  }

  registerPeer(peerId: string): void {
    if (this.registered.has(peerId)) return
    this.graph.addInput(peerId)
    trackClientId(getGraphGroupState(this.graph), peerId)
    this.registered.add(peerId)
    this.peers.set(peerId, this.createPeerMixState())
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

  /** @internal Post-mute mix burst — one mixed frame per call, bypasses due accounting. */
  async burstOutboundMix(peerId: string, frames = MIX_MUTE_FLUSH_FRAMES): Promise<void> {
    for (let i = 0; i < frames; i++) {
      await this.enqueueBurstMixFrame(peerId)
    }
  }

  /**
   * Real-time paced post-pose burst so WebRTC playout drains pre-flip pan before new TTS.
   * Fast back-to-back writes queue silence in the sender but do not advance listener playout.
   */
  private async pacedBurstOutboundMix(peerId: string, frames: number): Promise<void> {
    for (let i = 0; i < frames; i++) {
      await this.enqueueBurstMixFrame(peerId)
      if (i + 1 < frames) {
        await delayMs(PCM_FRAME_DURATION_MS)
      }
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
    state.pumpT0 = performance.now()
    state.pumpWritten = 0
    state.consecutiveTtsSkips = 0
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

  setSourceMixPlacement(participantId: string, placement: MixPlacement): void {
    this.graph.setSourceMixPlacement?.(participantId, placement)
  }

  clearSourceMixPlacement(participantId: string): void {
    this.graph.clearSourceMixPlacement?.(participantId)
  }

  setTtsMixPlacement(placement: MixPlacement): void {
    this.graph.setTtsMixPlacement(placement)
  }

  /**
   * Pause pump, drain in-flight writes, apply graph pose, then burst post-pose mix so
   * WebRTC playout cannot deliver pre-flip pan into the next utterance RMS window.
   */
  private async drainMixAfterTtsPoseChange(
    clientId: string,
    applyPose: () => void,
    frames = MIX_POSE_FLUSH_FRAMES,
  ): Promise<void> {
    const state = this.peers.get(clientId)
    if (!state) {
      applyPose()
      return
    }
    await this.pauseMixPump(clientId)
    state.ttsQueue.length = 0
    state.sidecarLeftover = Buffer.alloc(0)
    state.lastSidecarEnqueueAt = null
    state.consecutiveTtsSkips = 0
    applyPose()
    try {
      await this.pacedBurstOutboundMix(clientId, frames)
      await (this.pumpWrites.get(clientId) ?? Promise.resolve())
    } finally {
      this.resumeMixPump(clientId)
    }
  }

  /** Clears pending TTS and flushes post-pose silence on the listener outbound (WebRTC playout drain). */
  async setTtsPose(clientId: string, pose: ClientPose): Promise<void> {
    await this.drainMixAfterTtsPoseChange(clientId, () => this.graph.setTtsPose(clientId, pose))
  }

  async clearTtsPose(clientId: string): Promise<void> {
    await this.drainMixAfterTtsPoseChange(clientId, () => this.graph.clearTtsPose(clientId))
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
      if (pcm == null) return
      const combined =
        state.sidecarLeftover.length > 0 ? Buffer.concat([state.sidecarLeftover, pcm]) : pcm
      let offset = 0
      while (offset + PCM_FULL_FRAME_BYTES <= combined.length) {
        this.enqueueTtsFrame(
          state,
          Buffer.from(combined.subarray(offset, offset + PCM_FULL_FRAME_BYTES)),
        )
        offset += PCM_FULL_FRAME_BYTES
      }
      state.sidecarLeftover =
        offset < combined.length ? Buffer.from(combined.subarray(offset)) : Buffer.alloc(0)
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
   * Starts a 20 ms wall-clock pump that is the sole writer to the PC outbound track:
   * `sum(panTtsFrame(ttsOrSilence), renderOutput(listener))`.
   */
  startMixPump(peerId: string, pcOutbound: MixPumpOutboundTrack): void {
    const state = this.peerState(peerId)
    if (state.pumpInterval) return

    state.pcOutbound = pcOutbound
    state.pumpT0 = performance.now()
    state.pumpWritten = 0
    state.consecutiveTtsSkips = 0
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

  private enqueueBurstMixFrame(peerId: string): Promise<void> {
    const prev = this.pumpWrites.get(peerId) ?? Promise.resolve()
    const next = prev.catch(() => undefined).then(() => this.writeBurstMixFrame(peerId))
    this.pumpWrites.set(peerId, next)
    return next
  }

  private enqueueTtsFrame(state: PeerMixState, frame: Buffer): void {
    if (state.ttsQueue.length >= TTS_QUEUE_MAX_FRAMES) {
      state.ttsQueue.shift()
      state.ttsQueueOverflowDrops += 1
    }
    state.ttsQueue.push(frame)
    state.lastSidecarEnqueueAt = performance.now()
    state.consecutiveTtsSkips = 0
  }

  private tryDequeueTts(state: PeerMixState): Buffer | null {
    return state.ttsQueue.shift() ?? null
  }

  private isTtsStreamActive(state: PeerMixState, now: number): boolean {
    if (state.queueWasNonEmptyLastTick) return true
    if (state.lastSidecarEnqueueAt == null) return false
    return now - state.lastSidecarEnqueueAt <= TTS_ACTIVE_WINDOW_MS
  }

  private async writeMixTick(
    peerId: string,
    out: MixPumpOutboundTrack,
    tts: Buffer,
  ): Promise<void> {
    const mixed = this.graph.renderOutput(peerId)
    const panned = this.graph.panTtsFrame(tts, peerId)
    const frame = sumStereoPcm(panned, mixed)
    await out.writeSample(frame, PCM_FRAME_DURATION_MS)
  }

  /** @internal One burst frame (mute/pose flush) — bypasses wall-clock due slots. */
  private async writeBurstMixFrame(peerId: string): Promise<void> {
    const state = this.peers.get(peerId)
    if (!state?.pcOutbound) return
    const tts = this.tryDequeueTts(state) ?? this.silenceFrame
    await this.writeMixTick(peerId, state.pcOutbound, tts)
  }

  /** @internal One wall-clock pump tick — exposed for unit tests with fake timers. */
  async pumpMixFrame(peerId: string, pcOutbound?: MixPumpOutboundTrack): Promise<void> {
    const state = this.peers.get(peerId)
    if (!state) return
    const out = pcOutbound ?? state.pcOutbound
    if (!out) return

    const now = performance.now()
    let due = Math.floor((now - state.pumpT0) / PCM_FRAME_DURATION_MS) - state.pumpWritten
    if (due <= 0) return

    const originalDue = due
    const slotBudget = Math.min(due, PUMP_MAX_FRAMES_PER_TICK)
    let slotsAttempted = 0

    while (slotsAttempted < slotBudget && due > 0) {
      const tts = this.tryDequeueTts(state)
      if (tts != null) {
        await this.writeMixTick(peerId, out, tts)
        state.pumpWritten += 1
        state.consecutiveTtsSkips = 0
        due -= 1
        slotsAttempted += 1
        continue
      }

      if (
        this.isTtsStreamActive(state, now) &&
        state.consecutiveTtsSkips < TTS_MAX_CONSECUTIVE_SKIPS
      ) {
        state.consecutiveTtsSkips += 1
        state.pumpWritten += 1
        due -= 1
        slotsAttempted += 1
        continue
      }

      await this.writeMixTick(peerId, out, this.silenceFrame)
      state.pumpWritten += 1
      state.consecutiveTtsSkips = 0
      due -= 1
      slotsAttempted += 1
    }

    state.queueWasNonEmptyLastTick = state.ttsQueue.length > 0

    if (originalDue > PUMP_MAX_FRAMES_PER_TICK) {
      this.rebasePumpClock(state, now)
    }
  }
}
