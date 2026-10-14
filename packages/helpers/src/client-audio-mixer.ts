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
import {
  AudioMixGraph,
  type ClientPose,
  type MixPlacement,
  type MixStats,
  type NativePumpStats,
} from '@node-webrtc-rust/sdk/mix'
import type { RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from './pcm.js'
import { pcmFromWriteSampleTeeArgs } from './session-recorder.js'

const WRAPPED_IN = Symbol('clientAudioMixerInbound')

function delayMs(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** Post-mute outbound bursts (~380ms) to replace pre-mute WebRTC playout buffer. */
const MIX_MUTE_FLUSH_FRAMES = 19
/** Sidecar TTS backlog (~1 s) above which the pump warns. The FIFO is unbounded: TTS frames are never dropped. */
const TTS_BACKLOG_WARN_FRAMES = 50
/** Max outbound frames emitted from one interval tick (avoids silence catch-up bursts). */
const PUMP_MAX_FRAMES_PER_TICK = 4
/** Sidecar enqueue within this window keeps the TTS stream "active" (skip silence on empty queue). */
const TTS_ACTIVE_WINDOW_MS = 60
/** After this many consecutive active-window skips (~60 ms), treat TTS as ended and write silence. */
const TTS_MAX_CONSECUTIVE_SKIPS = 3
/** How often the pump compares each source's queue counters with the previous snapshot. */
const QUEUE_STATS_CHECK_INTERVAL_MS = 1_000
/** Minimum spacing between queue-pressure warnings for one source. */
const QUEUE_WARN_MIN_INTERVAL_MS = 10_000

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
  /**
   * Renders one 20 ms mixed frame. `atMs` is the scheduled render time on the
   * `performance.now()` clock; renders 20 ms apart play consecutive frames.
   */
  renderOutput(listenerId: string, atMs?: number): Buffer
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
  /** Per-source queue counters; `null` when the source is unknown. */
  mixStats?(participantId: string): MixStats | null | undefined
  /** Native mix pump (Rust task per listener); absent on mock graphs. */
  startNativePump?(
    listenerId: string,
    pcOutbound: LocalAudioTrack,
    ttsSidecar: LocalAudioTrack,
  ): Promise<void>
  stopNativePump?(listenerId: string, ttsSidecar?: LocalAudioTrack): void
  pauseNativePump?(listenerId: string): Promise<void>
  resumeNativePump?(listenerId: string): void
  burstNativePump?(listenerId: string, frames: number, paced: boolean): Promise<void>
  discardNativeTts?(listenerId: string): number
  nativePumpStats?(listenerId: string): NativePumpStats | null | undefined
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

/**
 * Sink for mixer warnings. `message` is the full single-line text (legacy format, so a
 * one-argument sink keeps every counter); `fields` carries the same values structured.
 * Queue-pressure fields: `{ peerId, compressed, concealed, overflowCompressed, decodeErrors,
 * listenerCatchupCompressed, listenerSkipped, depth, target }` (counter deltas since the last
 * warning, as numbers; depth and target are current values).
 */
export type ClientMixerWarn = (message: string, fields?: Record<string, unknown>) => void

/** Single-line text for the default `console.warn` sink (keeps stdout greps working). */
export function formatQueuePressureLine(message: string, fields: Record<string, unknown>): string {
  if (fields.depthMs !== undefined) {
    // TTS backlog warning: only peerId + depth fields.
    return (
      `${message} peerId=${String(fields.peerId)} depthMs=${String(fields.depthMs)} ` +
      `maxDepthMs=${String(fields.maxDepthMs)}`
    )
  }
  return (
    `${message} peerId=${String(fields.peerId)} compressed=+${String(fields.compressed)} ` +
    `concealed=+${String(fields.concealed)} overflowCompressed=+${String(fields.overflowCompressed)} ` +
    `decodeErrors=+${String(fields.decodeErrors)} depth=${String(fields.depth)} target=${String(fields.target)} ` +
    `listenerCatchupCompressed=+${String(fields.listenerCatchupCompressed)} ` +
    `listenerSkipped=+${String(fields.listenerSkipped)}`
  )
}

export type ClientAudioMixerOptions = {
  /** Inject a mock graph in unit tests; defaults to a new {@link AudioMixGraph}. */
  graph?: ClientMixGraph
  /**
   * Sink for rate-limited queue-pressure warnings. `message` is the full single-line text
   * (legacy format); `fields` is the same values structured:
   * `{ peerId, compressed, concealed, overflowCompressed, decodeErrors, listenerCatchupCompressed,
   * listenerSkipped, depth, target }`.
   * Default prints the message to `console.warn`.
   */
  warn?: ClientMixerWarn
  /**
   * Run the 20 ms mix pump as a native Rust task per listener (default `true`). Also needs a
   * native graph and native tracks, and `NWR_MIX_NATIVE_PUMP` not set to `0`; otherwise the
   * JS pump runs.
   */
  nativePump?: boolean
}

/** Counters whose increase means audio was compressed, concealed or failed to decode. */
type QueuePressureCounters = Pick<
  MixStats,
  | 'compressed'
  | 'concealed'
  | 'overflowCompressed'
  | 'decodeErrors'
  | 'listenerCatchupCompressed'
  | 'listenerSkipped'
>

type QueueStatsWatch = {
  /** Counters at the last warning (or zero before the first one). */
  baseline: QueuePressureCounters
  lastWarnAt: number | null
}

const ZERO_QUEUE_PRESSURE: QueuePressureCounters = {
  compressed: 0,
  concealed: 0,
  overflowCompressed: 0,
  decodeErrors: 0,
  listenerCatchupCompressed: 0,
  listenerSkipped: 0,
}

type PeerMixState = {
  /** Full sidecar TTS frames queued for the mix pump. */
  ttsQueue: Buffer[]
  /** Deepest sidecar TTS backlog (frames) since the last backlog warning. */
  ttsQueueMaxDepth: number
  /** `performance.now()` of the last `[mix] tts backlog` warning for this peer. */
  ttsBacklogWarnAt: number | null
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
  /** The native Rust pump owns this peer's outbound; the JS pump and `ttsQueue` stay unused. */
  nativePump: boolean
  /** Pending `startNativePump` call (control calls wait for it). */
  nativeStart?: Promise<void>
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
  private readonly warn: ClientMixerWarn
  private readonly queueWatch = new Map<string, QueueStatsWatch>()
  private lastQueueCheckAt: number | null = null
  private readonly nativePumpEnabled: boolean
  /** Shared 1 Hz poll for native pump stats and queue pressure. */
  private nativeStatsTimer?: ReturnType<typeof setInterval>

  constructor(options?: ClientAudioMixerOptions) {
    this.graph = options?.graph ?? new AudioMixGraph()
    this.warn = options?.warn ?? ((message) => console.warn(message))
    this.nativePumpEnabled = options?.nativePump !== false
  }

  /** Per-source mix queue counters (`null` when the graph has no stats or the source is unknown). */
  mixStats(peerId: string): MixStats | null {
    return this.graph.mixStats?.(peerId) ?? null
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
      ttsQueueMaxDepth: 0,
      ttsBacklogWarnAt: null,
      sidecarLeftover: Buffer.alloc(0),
      pumpT0: performance.now(),
      pumpWritten: 0,
      lastSidecarEnqueueAt: null,
      queueWasNonEmptyLastTick: false,
      consecutiveTtsSkips: 0,
      nativePump: false,
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
    this.queueWatch.delete(peerId)
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
    const native = this.peers.get(peerId)
    if (native?.nativePump) {
      await native.nativeStart?.catch(() => undefined)
      await this.graph.burstNativePump?.(peerId, frames, false)
      return
    }
    for (let i = 0; i < frames; i++) {
      await this.enqueueBurstMixFrame(peerId)
    }
  }

  /**
   * Real-time paced post-pose burst so WebRTC playout drains pre-flip pan before new TTS.
   * Fast back-to-back writes queue silence in the sender but do not advance listener playout.
   */
  private async pacedBurstOutboundMix(peerId: string, frames: number): Promise<void> {
    const native = this.peers.get(peerId)
    if (native?.nativePump) {
      await native.nativeStart?.catch(() => undefined)
      await this.graph.burstNativePump?.(peerId, frames, true)
      return
    }
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
    if (state.nativePump) {
      await state.nativeStart?.catch(() => undefined)
      await this.graph.pauseNativePump?.(peerId)
      return
    }
    if (state.pumpInterval) {
      clearInterval(state.pumpInterval)
      state.pumpInterval = undefined
    }
    await (this.pumpWrites.get(peerId) ?? Promise.resolve())
  }

  /** @internal Resume one listener mix pump after {@link pauseMixPump}. */
  resumeMixPump(peerId: string): void {
    const state = this.peers.get(peerId)
    if (state?.nativePump) {
      this.graph.resumeNativePump?.(peerId)
      return
    }
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

  /** Updates TTS pan pose on the mix graph; queued sidecar PCM is panned at mix time. */
  async setTtsPose(clientId: string, pose: ClientPose): Promise<void> {
    this.graph.setTtsPose(clientId, pose)
  }

  async clearTtsPose(clientId: string): Promise<void> {
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
      if (pcm == null) return
      const combined =
        state.sidecarLeftover.length > 0 ? Buffer.concat([state.sidecarLeftover, pcm]) : pcm
      let offset = 0
      while (offset + PCM_FULL_FRAME_BYTES <= combined.length) {
        this.enqueueTtsFrame(
          peerId,
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
    if (state.pumpInterval || state.nativePump) return

    if (this.canUseNativePump(state, pcOutbound)) {
      this.startNativeMixPump(peerId, state, pcOutbound, state.sidecar)
      return
    }

    state.pcOutbound = pcOutbound
    state.pumpT0 = performance.now()
    state.pumpWritten = 0
    state.consecutiveTtsSkips = 0
    state.pumpInterval = setInterval(() => {
      void this.enqueuePumpMixFrame(peerId, pcOutbound).catch(() => undefined)
    }, PCM_FRAME_DURATION_MS)
  }

  private canUseNativePump(
    state: PeerMixState,
    pcOutbound: MixPumpOutboundTrack,
  ): pcOutbound is LocalAudioTrack {
    return (
      this.nativePumpEnabled &&
      process.env.NWR_MIX_NATIVE_PUMP !== '0' &&
      typeof this.graph.startNativePump === 'function' &&
      pcOutbound instanceof LocalAudioTrack &&
      state.sidecar instanceof LocalAudioTrack
    )
  }

  private startNativeMixPump(
    peerId: string,
    state: PeerMixState,
    pcOutbound: LocalAudioTrack,
    sidecar: TtsSidecarTrack | undefined,
  ): void {
    // The native sink replaces the JS tee: TTS goes straight into the native queue.
    sidecar?.setWriteSampleTee(null)
    state.nativePump = true
    state.pcOutbound = pcOutbound
    const start = this.graph.startNativePump!(peerId, pcOutbound, sidecar as LocalAudioTrack)
    state.nativeStart = start
    start.catch((err: unknown) => {
      this.warn(`[mix] native pump start failed peerId=${peerId} error=${String(err)}`)
    })
    this.ensureNativeStatsTimer()
  }

  private ensureNativeStatsTimer(): void {
    if (this.nativeStatsTimer) return
    this.nativeStatsTimer = setInterval(() => this.pollNativePumps(), QUEUE_STATS_CHECK_INTERVAL_MS)
    this.nativeStatsTimer.unref()
  }

  private stopNativeStatsTimerIfIdle(): void {
    if (!this.nativeStatsTimer) return
    for (const state of this.peers.values()) {
      if (state.nativePump) return
    }
    clearInterval(this.nativeStatsTimer)
    this.nativeStatsTimer = undefined
  }

  private pollNativePumps(): void {
    const now = performance.now()
    this.checkQueueStats(now)
    for (const [peerId, state] of this.peers) {
      if (!state.nativePump) continue
      const stats = this.graph.nativePumpStats?.(peerId)
      if (!stats) continue
      if (stats.ttsQueueMaxDepthFrames > TTS_BACKLOG_WARN_FRAMES) {
        this.maybeWarnTtsBacklog(
          peerId,
          state,
          now,
          stats.ttsQueueFrames,
          stats.ttsQueueMaxDepthFrames,
        )
      }
    }
  }

  private stopNativeMixPump(peerId: string, state: PeerMixState): void {
    const sidecar = state.sidecar instanceof LocalAudioTrack ? state.sidecar : undefined
    this.graph.stopNativePump?.(peerId, sidecar)
    const start = state.nativeStart
    if (start) {
      // A start still in flight installs its sink late; stop again once it has settled.
      void start.catch(() => undefined).then(() => this.graph.stopNativePump?.(peerId, sidecar))
    }
    state.nativeStart = undefined
    state.nativePump = false
    state.pcOutbound = undefined
    // Back to the JS tee so a later JS pump sees sidecar TTS again.
    if (state.sidecar) this.wireTtsSidecar(peerId, state.sidecar)
    this.stopNativeStatsTimerIfIdle()
  }

  stopMixPump(peerId: string): void {
    const state = this.peers.get(peerId)
    if (state?.nativePump) {
      this.stopNativeMixPump(peerId, state)
      return
    }
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

  private enqueueTtsFrame(peerId: string, state: PeerMixState, frame: Buffer): void {
    state.ttsQueue.push(frame)
    state.lastSidecarEnqueueAt = performance.now()
    state.consecutiveTtsSkips = 0
    const depth = state.ttsQueue.length
    if (depth > state.ttsQueueMaxDepth) state.ttsQueueMaxDepth = depth
    if (
      depth > TTS_BACKLOG_WARN_FRAMES &&
      this.maybeWarnTtsBacklog(
        peerId,
        state,
        state.lastSidecarEnqueueAt,
        depth,
        state.ttsQueueMaxDepth,
      )
    ) {
      state.ttsQueueMaxDepth = depth
    }
  }

  /** Throttled (QUEUE_WARN_MIN_INTERVAL_MS per peer) warning: the pump is behind the TTS producer. Frames are kept, never dropped. */
  private maybeWarnTtsBacklog(
    peerId: string,
    state: PeerMixState,
    now: number,
    depthFrames: number,
    maxDepthFrames: number,
  ): boolean {
    if (state.ttsBacklogWarnAt != null && now - state.ttsBacklogWarnAt < QUEUE_WARN_MIN_INTERVAL_MS)
      return false
    state.ttsBacklogWarnAt = now
    const fields = {
      peerId,
      depthMs: depthFrames * PCM_FRAME_DURATION_MS,
      maxDepthMs: maxDepthFrames * PCM_FRAME_DURATION_MS,
    }
    this.warn(formatQueuePressureLine('[mix] tts backlog', fields), fields)
    return true
  }

  /**
   * Discards queued sidecar TTS for one peer (barge-in / flush). Returns the number of frames discarded.
   * The agent stopped this reply; its queued frames must not play after the barge-in.
   */
  discardQueuedTts(peerId: string): number {
    const state = this.peers.get(peerId)
    if (!state) return 0
    if (state.nativePump) return this.graph.discardNativeTts?.(peerId) ?? 0
    const n = state.ttsQueue.length
    state.ttsQueue = []
    state.sidecarLeftover = Buffer.alloc(0)
    state.queueWasNonEmptyLastTick = false
    return n
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
    slotMs?: number,
  ): Promise<void> {
    const mixed = this.graph.renderOutput(peerId, slotMs)
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
    this.checkQueueStats(now)
    let due = Math.floor((now - state.pumpT0) / PCM_FRAME_DURATION_MS) - state.pumpWritten
    if (due <= 0) return

    const originalDue = due
    const slotBudget = Math.min(due, PUMP_MAX_FRAMES_PER_TICK)
    let slotsAttempted = 0

    while (slotsAttempted < slotBudget && due > 0) {
      const tts = this.tryDequeueTts(state)
      // Scheduled time of this slot: a catch-up render in the same tick gets the next
      // slot's time, so the graph plays the next frame instead of repeating the last.
      const slotMs = state.pumpT0 + (state.pumpWritten + 1) * PCM_FRAME_DURATION_MS
      if (tts != null) {
        await this.writeMixTick(peerId, out, tts, slotMs)
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

      await this.writeMixTick(peerId, out, this.silenceFrame, slotMs)
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

  /**
   * At most once per {@link QUEUE_STATS_CHECK_INTERVAL_MS}, compares every registered source's
   * counters with the last warned snapshot and logs one line per source when compression,
   * concealment, overflow merging or decode errors increased (at most one line per
   * {@link QUEUE_WARN_MIN_INTERVAL_MS} per source; increases in between are folded into the
   * next line).
   */
  private checkQueueStats(now: number): void {
    if (!this.graph.mixStats) return
    if (
      this.lastQueueCheckAt != null &&
      now - this.lastQueueCheckAt < QUEUE_STATS_CHECK_INTERVAL_MS
    ) {
      return
    }
    this.lastQueueCheckAt = now
    for (const peerId of this.registered) {
      const stats = this.graph.mixStats(peerId)
      if (!stats) continue
      let watch = this.queueWatch.get(peerId)
      if (!watch) {
        watch = { baseline: ZERO_QUEUE_PRESSURE, lastWarnAt: null }
        this.queueWatch.set(peerId, watch)
      }
      if (stats.played === 0) {
        // Start-up: concealment before the source's first frame is expected, not pressure.
        watch.baseline = { ...watch.baseline, concealed: stats.concealed }
      }
      const delta = {
        compressed: stats.compressed - watch.baseline.compressed,
        concealed: stats.concealed - watch.baseline.concealed,
        overflowCompressed: stats.overflowCompressed - watch.baseline.overflowCompressed,
        decodeErrors: stats.decodeErrors - watch.baseline.decodeErrors,
        listenerCatchupCompressed:
          stats.listenerCatchupCompressed - watch.baseline.listenerCatchupCompressed,
        listenerSkipped: stats.listenerSkipped - watch.baseline.listenerSkipped,
      }
      if (
        delta.compressed <= 0 &&
        delta.concealed <= 0 &&
        delta.overflowCompressed <= 0 &&
        delta.decodeErrors <= 0 &&
        delta.listenerCatchupCompressed <= 0 &&
        delta.listenerSkipped <= 0
      ) {
        continue
      }
      if (watch.lastWarnAt != null && now - watch.lastWarnAt < QUEUE_WARN_MIN_INTERVAL_MS) {
        continue
      }
      watch.lastWarnAt = now
      watch.baseline = {
        compressed: stats.compressed,
        concealed: stats.concealed,
        overflowCompressed: stats.overflowCompressed,
        decodeErrors: stats.decodeErrors,
        listenerCatchupCompressed: stats.listenerCatchupCompressed,
        listenerSkipped: stats.listenerSkipped,
      }
      const fields = {
        peerId,
        compressed: delta.compressed,
        concealed: delta.concealed,
        overflowCompressed: delta.overflowCompressed,
        decodeErrors: delta.decodeErrors,
        listenerCatchupCompressed: delta.listenerCatchupCompressed,
        listenerSkipped: delta.listenerSkipped,
        depth: stats.depth,
        target: stats.target,
      }
      this.warn(formatQueuePressureLine('[mix] queue pressure', fields), fields)
    }
  }
}
