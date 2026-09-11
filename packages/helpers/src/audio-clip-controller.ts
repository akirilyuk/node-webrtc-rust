/**
 * Routes native clip PCM into MixGraph (voice+data / mix pump) or direct outbound overlay
 * (voice-only without a mix pump). One global 20 ms tick advances every active play clock.
 */

import {
  getClip,
  stopClip,
  takeClipFrame,
  type ClipPlayerStatus,
} from '@node-webrtc-rust/sdk/player'

import { assertAudioPositionExclusive, type AudioPosition } from './audio-position.js'
import {
  sumStereoPcm,
  type ClientAudioMixer,
  type ClientMixGraph,
  type MixPumpOutboundTrack,
} from './client-audio-mixer.js'
import {
  clipPlayInputId,
  scaleStereoPcmVolume,
  startClipPlayback,
  type AudioPlaySource,
} from './clip-playback.js'
import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from './pcm.js'
import { pcmFromWriteSampleTeeArgs } from './session-recorder.js'

type RouteSnapshot = {
  listenerId: string
  hadExplicit: boolean
  previousSources: string[] | null
}

type ActivePlay = {
  playId: string
  mixInputId: string
  targetPeerIds: Set<string>
  volume: number
  routeSnapshots: RouteSnapshot[]
  usesMixGraph: boolean
}

type DirectOverlayPeer = {
  pendingTts: Buffer | null
  pendingClip: Buffer | null
  pumpInterval?: ReturnType<typeof setInterval>
  outbound: MixPumpOutboundTrack
  writeChain: Promise<void>
}

export type AudioClipControllerOptions = {
  cacheDir?: string
  fetch?: (url: string) => Promise<Uint8Array>
}

export type PlayAudioRequest = {
  source: AudioPlaySource
  peerIds?: string[]
  volume?: number
  position?: AudioPosition
}

export type { AudioPosition } from './audio-position.js'

export type AudioClipPlayDeps = {
  resolveTargetPeerIds: (peerIds?: string[]) => string[]
  getMixer: () => ClientAudioMixer | undefined
  listRegisteredPeers: () => string[]
  getDirectOutbound: (peerId: string) => MixPumpOutboundTrack | undefined
  usesMixPump: (peerId: string) => boolean
}

export class AudioClipController {
  private readonly plays = new Map<string, ActivePlay>()
  private readonly directOverlays = new Map<string, DirectOverlayPeer>()
  private tickInterval?: ReturnType<typeof setInterval>
  private readonly silence = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  private readonly options: AudioClipControllerOptions
  private getMixer: () => ClientAudioMixer | undefined = () => undefined

  constructor(options?: AudioClipControllerOptions) {
    this.options = options ?? {}
  }

  /** Wire host mixer lookup (called once from {@link VoiceAgentSessionHost}). */
  bindMixer(getMixer: () => ClientAudioMixer | undefined): void {
    this.getMixer = getMixer
  }

  get activePlayCount(): number {
    return this.plays.size
  }

  async play(request: PlayAudioRequest, deps: AudioClipPlayDeps): Promise<{ playId: string }> {
    const targetPeerIds = deps.resolveTargetPeerIds(request.peerIds)
    const volume = clampVolume(request.volume)
    const playId = await startClipPlayback(request.source, {
      cacheDir: this.options.cacheDir,
      fetch: this.options.fetch,
    })
    const mixInputId = clipPlayInputId(playId)
    const mixer = deps.getMixer()
    const graph = mixer?.getMixGraph()
    const usesMixGraph = graph != null && targetPeerIds.some((id) => deps.usesMixPump(id))

    const routeSnapshots: RouteSnapshot[] = []
    if (usesMixGraph && graph) {
      graph.addInput(mixInputId)
      this.applyPlayPosition(graph, mixInputId, request.position)
      this.applyTargetPlayRoutes(
        routeSnapshots,
        graph,
        deps.listRegisteredPeers(),
        mixInputId,
        targetPeerIds,
      )
    }

    for (const peerId of targetPeerIds) {
      if (!deps.usesMixPump(peerId)) {
        const outbound = deps.getDirectOutbound(peerId)
        if (outbound) {
          this.ensureDirectOverlay(peerId, outbound)
        }
      }
    }

    this.plays.set(playId, {
      playId,
      mixInputId,
      targetPeerIds: new Set(targetPeerIds),
      volume,
      routeSnapshots,
      usesMixGraph,
    })
    this.ensureTick()
    return { playId }
  }

  getPlay(playId: string): ClipPlayerStatus | undefined {
    return getClip(playId)
  }

  stop(playId: string): boolean {
    const play = this.plays.get(playId)
    if (!play) {
      return stopClip(playId)
    }
    this.teardownPlay(play)
    return stopClip(playId)
  }

  onPeerDisconnected(peerId: string, deps: Pick<AudioClipPlayDeps, 'listRegisteredPeers'>): void {
    this.stopDirectOverlay(peerId)
    for (const play of [...this.plays.values()]) {
      if (!play.targetPeerIds.has(peerId)) continue
      play.targetPeerIds.delete(peerId)
      if (play.targetPeerIds.size === 0) {
        this.teardownPlay(play)
        stopClip(play.playId)
        continue
      }
      if (play.usesMixGraph) {
        this.refreshPlayRoutes(play, this.getMixer()?.getMixGraph(), deps.listRegisteredPeers())
      }
    }
  }

  close(): void {
    if (this.tickInterval) {
      clearInterval(this.tickInterval)
      this.tickInterval = undefined
    }
    for (const peerId of [...this.directOverlays.keys()]) {
      this.stopDirectOverlay(peerId)
    }
    while (this.plays.size > 0) {
      const play = this.plays.values().next().value!
      this.teardownPlay(play)
      stopClip(play.playId)
    }
  }

  private ensureTick(): void {
    if (this.tickInterval) return
    this.tickInterval = setInterval(() => {
      void this.tick()
    }, PCM_FRAME_DURATION_MS)
  }

  private stopTickIfIdle(): void {
    if (this.plays.size > 0 || this.directOverlays.size > 0) return
    if (this.tickInterval) {
      clearInterval(this.tickInterval)
      this.tickInterval = undefined
    }
  }

  private tick(): void {
    const graph = this.getMixer()?.getMixGraph()
    for (const play of [...this.plays.values()]) {
      const frame = takeClipFrame(play.playId)
      const status = getClip(play.playId)
      if (
        status?.status === 'ended' ||
        status?.status === 'error' ||
        status?.status === 'stopped'
      ) {
        this.teardownPlay(play)
        stopClip(play.playId)
        continue
      }

      const scaled =
        frame != null && frame.length === PCM_FULL_FRAME_BYTES
          ? scaleStereoPcmVolume(frame, play.volume)
          : null

      if (play.usesMixGraph && graph && scaled) {
        graph.pushFrame(play.mixInputId, scaled)
      }

      if (scaled) {
        for (const peerId of play.targetPeerIds) {
          const overlay = this.directOverlays.get(peerId)
          if (overlay) {
            overlay.pendingClip = scaled
            overlay.writeChain = overlay.writeChain
              .catch(() => undefined)
              .then(() => this.pumpDirectOverlay(peerId, overlay))
          }
        }
      }
    }
    this.stopTickIfIdle()
  }

  private teardownPlay(play: ActivePlay): void {
    const graph = this.getMixer()?.getMixGraph()
    this.plays.delete(play.playId)

    if (play.usesMixGraph && graph) {
      // Do not restore pre-play route snapshots while other plays are active: each play
      // captured routes before later plays appended their mixInputId (staging clip-playback
      // probe H while G ends). Strip only this play's input, then re-apply remaining plays.
      this.removePlayMixInputFromRoutes(play.mixInputId, graph)
      play.routeSnapshots.length = 0
      graph.removeInput(play.mixInputId)

      const registered = this.listRegisteredPeers()
      for (const remaining of this.plays.values()) {
        if (remaining.usesMixGraph) {
          this.reapplyPlayRoutes(remaining, graph, registered)
        }
      }
    }
    this.stopTickIfIdle()
  }

  private listRegisteredPeers(): string[] {
    return this.getMixer()?.listRegisteredPeers() ?? []
  }

  /** Drop one clip input from explicit listener routes (no-op when routes are implicit). */
  private removePlayMixInputFromRoutes(mixInputId: string, graph: ClientMixGraph): void {
    if (!graph.setListenerSources || !graph.listenerSources) return
    for (const listenerId of this.listRegisteredPeers()) {
      const current = graph.listenerSources(listenerId)
      if (current == null || !current.includes(mixInputId)) continue
      graph.setListenerSources(
        listenerId,
        current.filter((source) => source !== mixInputId),
      )
    }
  }

  private applyPlayPosition(
    graph: ClientMixGraph,
    mixInputId: string,
    position?: AudioPosition,
  ): void {
    if (!position) return
    assertAudioPositionExclusive(position)
    if (position.placement != null) {
      graph.setSourceMixPlacement?.(mixInputId, position.placement)
      return
    }
    if (position.pose != null) {
      graph.setPose(mixInputId, position.pose)
      graph.setPositionalEnabled(true)
    }
  }

  private refreshPlayRoutes(
    play: ActivePlay,
    graph: ClientMixGraph | undefined,
    registered: string[],
  ): void {
    if (!graph?.setListenerSources) return
    this.reapplyPlayRoutes(play, graph, registered)
  }

  /** Recompute this play's listener routes from current graph state (not stale snapshots). */
  private reapplyPlayRoutes(play: ActivePlay, graph: ClientMixGraph, registered: string[]): void {
    play.routeSnapshots.length = 0
    this.applyTargetPlayRoutes(play.routeSnapshots, graph, registered, play.mixInputId, [
      ...play.targetPeerIds,
    ])
  }

  /**
   * When targeting a subset of mix peers, append `mixInputId` only to targeted listeners and
   * pin non-targets to explicit routes that omit the clip input (implicit hear-all would
   * still mix `addInput(clip)` into every listener).
   */
  private applyTargetPlayRoutes(
    routeSnapshots: RouteSnapshot[],
    graph: ClientMixGraph,
    registered: string[],
    mixInputId: string,
    targetPeerIds: string[],
  ): void {
    if (!graph.setListenerSources) return

    const targets = new Set(targetPeerIds)
    const subsetTargeting = targets.size > 0 && registered.some((peerId) => !targets.has(peerId))

    for (const listenerId of registered) {
      const current = graph.listenerSources?.(listenerId) ?? null
      const implicitBase = registered.filter((peerId) => peerId !== listenerId)
      const baseSources = current != null ? [...current] : implicitBase

      let nextSources: string[]
      if (targets.has(listenerId)) {
        nextSources = baseSources.includes(mixInputId) ? baseSources : [...baseSources, mixInputId]
      } else if (subsetTargeting) {
        nextSources = baseSources.filter((source) => source !== mixInputId)
      } else {
        continue
      }

      routeSnapshots.push({
        listenerId,
        hadExplicit: current != null,
        previousSources: current != null ? [...current] : null,
      })
      graph.setListenerSources(listenerId, nextSources)
    }
  }

  private ensureDirectOverlay(peerId: string, outbound: MixPumpOutboundTrack): void {
    if (this.directOverlays.has(peerId)) return
    this.directOverlays.set(peerId, {
      pendingTts: null,
      pendingClip: null,
      outbound,
      writeChain: Promise.resolve(),
    })
    this.ensureTick()
  }

  wireDirectOutboundTee(peerId: string, outbound: MixPumpOutboundTrack & TtsSidecarLike): void {
    const overlay = this.directOverlays.get(peerId)
    if (!overlay) return
    outbound.setWriteSampleTee((...args: unknown[]) => {
      const pcm = pcmFromWriteSampleTeeArgs(args)
      if (pcm != null && pcm.length === PCM_FULL_FRAME_BYTES) {
        overlay.pendingTts = Buffer.from(pcm)
      }
    })
    if (!overlay.pumpInterval) {
      overlay.pumpInterval = setInterval(() => {
        overlay.writeChain = overlay.writeChain
          .catch(() => undefined)
          .then(() => this.pumpDirectOverlay(peerId, overlay))
      }, PCM_FRAME_DURATION_MS)
    }
  }

  registerDirectOutbound(peerId: string, outbound: MixPumpOutboundTrack): void {
    this.ensureDirectOverlay(peerId, outbound)
  }

  private stopDirectOverlay(peerId: string): void {
    const overlay = this.directOverlays.get(peerId)
    if (!overlay) return
    if (overlay.pumpInterval) {
      clearInterval(overlay.pumpInterval)
    }
    this.directOverlays.delete(peerId)
    this.stopTickIfIdle()
  }

  private async pumpDirectOverlay(peerId: string, overlay: DirectOverlayPeer): Promise<void> {
    const clip = overlay.pendingClip ?? this.silence
    const tts = overlay.pendingTts ?? this.silence
    overlay.pendingClip = null
    overlay.pendingTts = null
    const frame = sumStereoPcm(tts, clip)
    await overlay.outbound.writeSample(frame, PCM_FRAME_DURATION_MS)
  }
}

type TtsSidecarLike = {
  setWriteSampleTee(callback: ((...args: unknown[]) => void) | null): void
}

function clampVolume(volume?: number): number {
  if (volume == null || Number.isNaN(volume)) return 1
  return Math.max(0, Math.min(1, volume))
}
