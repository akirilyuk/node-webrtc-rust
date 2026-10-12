import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, type MixStats } from '@node-webrtc-rust/sdk/mix'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { VoiceSessionBudget, resetProcessVoiceSessionBudget } from '../src/voice-session-budget.js'
import { connectAllOrCloseAll } from './connect-all-or-close.js'
import {
  defaultIceConfig,
  delay,
  FRAME_BYTES,
  waitForConnection,
} from './mix-three-client-helpers.js'
import {
  analyseSequence,
  detectIndex,
  FrameMarkerGenerator,
  type DetectedIndex,
  type SequenceAnalysis,
} from './support/frame-marker.js'

// Same-process loopback: do not set WEBRTC_NAT_1TO1_IPS (breaks ICE in CI).
//
// End-to-end audio continuity through the production path: SDK speaker client -> real Opus RTP
// -> SessionPod voice+data mix (MixGraph) -> real Opus RTP -> SDK listener client. The speaker
// sends frames carrying a frame-index marker (see support/frame-marker.ts); the listener decodes
// the marker of every 20 ms frame it reads and the sequence is checked for gaps and duplicates.

type SessionPodSlot = { sessionId: string; signaling: SignalingClient; host: VoiceAgentSessionHost }
type SessionPodTestAccess = SessionPod & { slots: Map<string, SessionPodSlot> }

function getVoiceHostForSession(
  pod: SessionPod,
  sessionId: string,
): VoiceAgentSessionHost | undefined {
  return (pod as SessionPodTestAccess).slots.get(sessionId)?.host
}

function nativeAvailable(): boolean {
  if (typeof RTCPeerConnection !== 'function') return false
  try {
    const graph = new AudioMixGraph()
    graph.addInput('probe')
    graph.removeInput('probe')
    return true
  } catch {
    return false
  }
}

// Client ids are unique per case so `waitForVoiceClientActive` can only be satisfied by the NEW
// connection's voice session, never by the previous case's one that is still being torn down.
const clientIds = (caseKey: string) => ({
  speaker: `client-fc-speaker-${caseKey}`,
  listener: `client-fc-listener-${caseKey}`,
})
const SPEAKER_SESSION = 'session-fc-speaker'
const LISTENER_SESSION = 'session-fc-listener'

const MARKED_FRAMES = 150
const FRAME_MS = 20
/** Trailing silent frames (after the last marker) that mean the stream has ended. */
const END_SILENT_FRAMES = 15
const DEADLINE_MS = 30_000

interface ConnectedClient {
  pc: RTCPeerConnection
  signaling: SignalingClient
  mic: LocalAudioTrack
  agentAudio: RemoteAudioTrack
  teardownNegotiate: () => void
}

async function connectClientToSession(
  wsUrl: string,
  sessionId: string,
  peerId: string,
): Promise<ConnectedClient> {
  const pc = new RTCPeerConnection(defaultIceConfig)
  const mic = new LocalAudioTrack(`${peerId}-mic`, `stream-${peerId}`)
  await pc.addTrack(mic)
  const signaling = new SignalingClient({ url: wsUrl, room: sessionId, peerId })
  const agentAudioPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out waiting for agent audio track (${peerId})`)),
      30_000,
    )
    pc.ontrack = (event) => {
      if (event.track instanceof RemoteAudioTrack) {
        clearTimeout(timer)
        resolve(event.track)
      }
    }
  })
  const teardownNegotiate = autoNegotiate({ pc, signaling, polite: true })
  await signaling.connect()
  await waitForConnection(pc)
  await mic.writeSample(Buffer.alloc(960), 5)
  const agentAudio = await agentAudioPromise
  return { pc, signaling, mic, agentAudio, teardownNegotiate }
}

function closeClient(client: ConnectedClient): void {
  client.teardownNegotiate()
  client.pc.close()
  client.signaling.disconnect()
}

async function waitForVoiceClientActive(
  pod: SessionPod,
  clientId: string,
  timeoutMs = 15_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    for (const sessionId of [SPEAKER_SESSION, LISTENER_SESSION]) {
      if (getVoiceHostForSession(pod, sessionId)?.isVoiceClientActive(clientId)) return
    }
    await delay(50)
  }
  throw new Error(`timed out waiting for active voice client ${clientId}`)
}

/** Deterministic PRNG (mulberry32) so jitter patterns are identical on every run. */
function seededRandom(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

type SendPattern = 'paced' | 'bunched-3x20' | 'packet-60ms' | 'jitter-30ms'

/** Streams MARKED_FRAMES marked frames through the speaker's real outbound track. */
async function sendMarkedFrames(mic: LocalAudioTrack, pattern: SendPattern): Promise<void> {
  const gen = new FrameMarkerGenerator()
  const frames = Array.from({ length: MARKED_FRAMES }, (_, i) => gen.next(i))
  const start = Date.now()
  // Coarse timer, then yield-spin for the last 2 ms so the send times are accurate to well
  // under a millisecond (a bare setTimeout can slip by several ms under load).
  const waitUntil = async (offsetMs: number) => {
    const target = start + offsetMs
    const coarse = target - 2 - Date.now()
    if (coarse > 0) await delay(coarse)
    while (Date.now() < target) await new Promise<void>((resolve) => setImmediate(resolve))
  }

  if (pattern === 'paced') {
    for (let i = 0; i < frames.length; i++) {
      await waitUntil(i * FRAME_MS)
      await mic.writeSample(frames[i]!, FRAME_MS)
    }
  } else if (pattern === 'bunched-3x20') {
    // Three 20 ms frames written back-to-back every 60 ms.
    for (let i = 0; i < frames.length; i += 3) {
      await waitUntil(i * FRAME_MS)
      for (let k = i; k < Math.min(i + 3, frames.length); k++) {
        await mic.writeSample(frames[k]!, FRAME_MS)
      }
    }
  } else if (pattern === 'packet-60ms') {
    // One 60 ms packet (3 frames, 11520 bytes) every 60 ms.
    for (let i = 0; i < frames.length; i += 3) {
      await waitUntil(i * FRAME_MS)
      await mic.writeSample(Buffer.concat(frames.slice(i, i + 3)), 3 * FRAME_MS)
    }
  } else {
    // Frame k is due at k*20 ms plus a seeded offset in [-30, +30] ms; order is preserved, so a
    // late frame is followed by an early burst.
    const rnd = seededRandom(0x5eed_f00d)
    let prevDue = 0
    for (let i = 0; i < frames.length; i++) {
      const due = Math.max(prevDue, i * FRAME_MS + Math.round((rnd() * 2 - 1) * 30))
      prevDue = due
      await waitUntil(due)
      await mic.writeSample(frames[i]!, FRAME_MS)
    }
  }
}

/**
 * Reads the listener mix until the marked stream has started and then ended (trailing
 * silence), or the deadline passes. Event-driven: no fixed sleeps as synchronisation.
 */
function startListenerRecorder(track: RemoteAudioTrack) {
  const detected: DetectedIndex[] = []
  let seenMarker = false
  let trailingSilent = 0
  let stopped = false
  let lastFrameAt = Date.now()
  let resolveEnded!: () => void
  let ended = new Promise<void>((resolve) => {
    resolveEnded = resolve
  })
  let resolveFirst!: () => void
  let firstMarker = new Promise<void>((resolve) => {
    resolveFirst = resolve
  })

  const loop = (async () => {
    while (!stopped) {
      const pcm = await track.readSample()
      lastFrameAt = Date.now()
      for (let off = 0; off + FRAME_BYTES <= pcm.length; off += FRAME_BYTES) {
        const idx = detectIndex(pcm.subarray(off, off + FRAME_BYTES))
        detected.push(idx)
        if (idx === 'silence') {
          trailingSilent++
        } else {
          trailingSilent = 0
          if (!seenMarker) {
            seenMarker = true
            resolveFirst()
          }
        }
        if (seenMarker && trailingSilent >= END_SILENT_FRAMES) resolveEnded()
      }
    }
  })().catch(() => undefined)

  return {
    detected,
    get firstMarker() {
      return firstMarker
    },
    get ended() {
      return ended
    },
    /**
     * Starts watching for another marked stream on the same track (after a pause). Returns
     * the index in `detected` where the new stream's slice begins.
     */
    nextStream(): number {
      seenMarker = false
      trailingSilent = 0
      ended = new Promise<void>((resolve) => {
        resolveEnded = resolve
      })
      firstMarker = new Promise<void>((resolve) => {
        resolveFirst = resolve
      })
      return detected.length
    },
    /** Resolves once no frame has arrived for `ms` (a stream with no idle frames has ended). */
    async quiet(ms: number): Promise<void> {
      while (Date.now() - lastFrameAt < ms) await delay(25)
    },
    async stop() {
      stopped = true
      track.stop()
      await Promise.race([loop, delay(500)])
    },
  }
}

async function withDeadline<T>(p: Promise<T>, label: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`${label}: timed out after ${DEADLINE_MS}ms`)),
      DEADLINE_MS,
    )
  })
  try {
    return await Promise.race([p, timeout])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

/**
 * Control: the same marked stream over a direct client-to-client Opus path (no mixer). It
 * shows the codec and the detector alone keep the sequence intact, so gaps seen in the
 * SessionPod cases come from the mix path.
 */
async function runDirectControl(pattern: SendPattern): Promise<SequenceAnalysis> {
  const server = new SignalingServer({ port: 0 })
  await server.listen(0, '127.0.0.1')
  const url = `ws://127.0.0.1:${server.port}`
  const room = `fc-control-${Date.now()}`
  const pcL = new RTCPeerConnection(defaultIceConfig)
  const pcS = new RTCPeerConnection(defaultIceConfig)
  const micL = new LocalAudioTrack('fc-ctl-l', 'stream-fc-ctl-l')
  const micS = new LocalAudioTrack('fc-ctl-s', 'stream-fc-ctl-s')
  await pcL.addTrack(micL)
  await pcS.addTrack(micS)
  const sigL = new SignalingClient({ url, room, peerId: 'fc-ctl-listener' })
  const sigS = new SignalingClient({ url, room, peerId: 'fc-ctl-speaker' })
  const listenerTrack = new Promise<RemoteAudioTrack>((resolve) => {
    pcL.ontrack = (event) => {
      if (event.track instanceof RemoteAudioTrack) resolve(event.track)
    }
  })
  const stopL = autoNegotiate({ pc: pcL, signaling: sigL, polite: true })
  const stopS = autoNegotiate({ pc: pcS, signaling: sigS, polite: false })
  try {
    await sigL.connect()
    await sigS.connect()
    await Promise.all([waitForConnection(pcL), waitForConnection(pcS)])
    await micS.writeSample(Buffer.alloc(960), 5)
    const track = await withDeadline(listenerTrack, 'control: remote track')
    const recorder = startListenerRecorder(track)
    try {
      const sending = sendMarkedFrames(micS, pattern)
      await withDeadline(recorder.firstMarker, 'control: first marker frame')
      await sending
      await withDeadline(
        Promise.race([recorder.ended, recorder.quiet(500)]),
        'control: end of marked stream',
      )
    } finally {
      await recorder.stop()
    }
    const result = analyseSequence(recorder.detected)
    console.log(`MIXSTATS ${JSON.stringify({ pattern: `control-${pattern}`, ...result })}`)
    return result
  } finally {
    stopL()
    stopS()
    pcL.close()
    pcS.close()
    sigL.disconnect()
    sigS.disconnect()
    await server.close?.()
  }
}

/**
 * Same bar for every send pattern: the elastic mix queue must not lose a frame.
 *
 * Bounds, measured over 10 runs per pattern on macOS:
 * - gaps 0, played >= 145: exactly three marker frames are always unrecognisable (index 39,
 *   2350 Hz, once per 40-frame period). They also come out ambiguous when the stream is relayed
 *   through two Opus hops without any mixer, so they are a codec artefact of this marker set,
 *   and an ambiguous frame stands in for its missing index (see `analyseSequence`).
 * - ambiguous <= 5: those three, plus one concealment frame at the stream edges.
 * - duplicates 0: the pump renders at its scheduled slot time (`renderOutput(id, atMs)`), so a
 *   late timer or a catch-up double render plays the next frame and never repeats the previous.
 */
function expectContinuous({ seq, stats }: { seq: SequenceAnalysis; stats: MixStats | null }): void {
  const detail = JSON.stringify({ seq, stats })
  expect(seq.gaps, detail).toBe(0)
  expect(seq.reordered, detail).toBe(0)
  expect(seq.duplicates, detail).toBe(0)
  expect(seq.played, detail).toBeGreaterThanOrEqual(145)
  expect(seq.ambiguous, detail).toBeLessThanOrEqual(5)
  expect(stats, detail).not.toBeNull()
  expect(stats!.decodeErrors, detail).toBe(0)
  expect(stats!.overflowCompressed, detail).toBe(0)
  expect(stats!.pushed, detail).toBeGreaterThanOrEqual(MARKED_FRAMES)
  expect(stats!.pushed, detail).toBe(
    stats!.played + stats!.compressed + stats!.overflowCompressed + stats!.depth,
  )
}

describe.skipIf(!nativeAvailable())('SessionPod mix frame continuity (JS to JS)', () => {
  let server: SignalingServer
  let wsUrl: string
  let pod: SessionPod

  beforeAll(async () => {
    resetProcessVoiceSessionBudget()
    server = new SignalingServer({ port: 0 })
    await server.listen(0, '127.0.0.1')
    wsUrl = `ws://127.0.0.1:${server.port}`
    pod = new SessionPod(server, {
      signalingUrl: wsUrl,
      iceServers: defaultIceConfig.iceServers,
      sessionMode: 'voice+data',
      teardownIdleSessions: false,
      voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } },
      sessionBudget: new VoiceSessionBudget(8),
      maxPreparedSessions: 8,
    })
    await pod.ensureSession(SPEAKER_SESSION)
    await pod.ensureSession(LISTENER_SESSION)
  })

  afterAll(async () => {
    await pod.close().catch(() => undefined)
    resetProcessVoiceSessionBudget()
  })

  /** `caseKey` names the clients and the mix group; it must be unique per case in this file. */
  async function runCase(
    pattern: SendPattern,
    caseKey: string = pattern,
  ): Promise<{ seq: SequenceAnalysis; stats: MixStats | null }> {
    const ids = clientIds(caseKey)
    const [speaker, listener] = await connectAllOrCloseAll(
      [
        () => connectClientToSession(wsUrl, SPEAKER_SESSION, ids.speaker),
        () => connectClientToSession(wsUrl, LISTENER_SESSION, ids.listener),
      ],
      closeClient,
    )
    try {
      // The mix graph is shared by both sessions. A non-positional group with both clients:
      // each participant hears the others (mix-minus), so the listener's mix is only the
      // speaker (the listener's own mic is silent).
      const host = getVoiceHostForSession(pod, LISTENER_SESSION)
      expect(host).toBeDefined()
      host!.createMixGroup({ id: `fc-${caseKey}`, clientIds: [ids.speaker, ids.listener] })
      await listener.mic.writeSample(Buffer.alloc(960), 5)
      await waitForVoiceClientActive(pod, ids.speaker)
      await waitForVoiceClientActive(pod, ids.listener)

      const recorder = startListenerRecorder(listener.agentAudio)
      try {
        const sending = sendMarkedFrames(speaker.mic, pattern)
        // Surface a rejected write (e.g. an unsupported packet size) instead of timing out.
        const sendFailed = sending.then(() => new Promise<never>(() => undefined))
        await withDeadline(
          Promise.race([recorder.firstMarker, sendFailed]),
          `${pattern}: first marker frame`,
        )
        await sending
        await withDeadline(
          Promise.race([recorder.ended, recorder.quiet(500)]),
          `${pattern}: end of marked stream`,
        )
      } finally {
        await recorder.stop()
        if (process.env.MIX_FC_DUMP) {
          const counts = new Map<string, number>()
          for (const d of recorder.detected) counts.set(String(d), (counts.get(String(d)) ?? 0) + 1)
          console.log(
            `RECORDED ${pattern} frames=${recorder.detected.length} ${JSON.stringify([...counts])}`,
          )
        }
      }
      const result = analyseSequence(recorder.detected)
      const stats =
        host!.getClientMixer()?.mixStats(host!.resolveParticipantId(ids.speaker)) ?? null
      // Machine-readable line for aggregating repeated runs.
      if (process.env.MIX_FC_DUMP) {
        console.log(`SEQ ${pattern} ${JSON.stringify(recorder.detected)}`)
      }
      console.log(`MIXSTATS ${JSON.stringify({ pattern, ...result, stats })}`)
      return { seq: result, stats }
    } finally {
      closeClient(speaker)
      closeClient(listener)
      await delay(200)
    }
  }

  /**
   * One speaker connection that streams a marked burst, stays silent for `pauseMs`, then
   * streams again under the SAME speaker (mix source) id. Returns one result per burst plus
   * the time from the first write of that burst to the first marker frame heard.
   */
  async function runPausedCase(
    pattern: SendPattern,
    caseKey: string,
    pauseMs: number,
  ): Promise<Array<{ seq: SequenceAnalysis; stats: MixStats | null; firstFrameMs: number }>> {
    const ids = clientIds(caseKey)
    const [speaker, listener] = await connectAllOrCloseAll(
      [
        () => connectClientToSession(wsUrl, SPEAKER_SESSION, ids.speaker),
        () => connectClientToSession(wsUrl, LISTENER_SESSION, ids.listener),
      ],
      closeClient,
    )
    try {
      const host = getVoiceHostForSession(pod, LISTENER_SESSION)
      expect(host).toBeDefined()
      host!.createMixGroup({ id: `fc-${caseKey}`, clientIds: [ids.speaker, ids.listener] })
      await listener.mic.writeSample(Buffer.alloc(960), 5)
      await waitForVoiceClientActive(pod, ids.speaker)
      await waitForVoiceClientActive(pod, ids.listener)

      const recorder = startListenerRecorder(listener.agentAudio)
      const results: Array<{
        seq: SequenceAnalysis
        stats: MixStats | null
        firstFrameMs: number
      }> = []
      try {
        let from = 0
        for (let burst = 0; burst < 2; burst++) {
          if (burst > 0) {
            await delay(pauseMs)
            from = recorder.nextStream()
          }
          const startedAt = Date.now()
          const sending = sendMarkedFrames(speaker.mic, pattern)
          const sendFailed = sending.then(() => new Promise<never>(() => undefined))
          await withDeadline(
            Promise.race([recorder.firstMarker, sendFailed]),
            `${pattern}: burst ${burst} first marker frame`,
          )
          const firstFrameMs = Date.now() - startedAt
          await sending
          await withDeadline(
            Promise.race([recorder.ended, recorder.quiet(500)]),
            `${pattern}: burst ${burst} end of marked stream`,
          )
          const seq = analyseSequence(recorder.detected.slice(from))
          const stats =
            host!.getClientMixer()?.mixStats(host!.resolveParticipantId(ids.speaker)) ?? null
          console.log(
            `MIXSTATS ${JSON.stringify({ pattern: `${caseKey}-burst${burst}`, ...seq, firstFrameMs, stats })}`,
          )
          results.push({ seq, stats, firstFrameMs })
        }
      } finally {
        await recorder.stop()
      }
      return results
    } finally {
      closeClient(speaker)
      closeClient(listener)
      await delay(200)
    }
  }

  it('control (no mixer): direct paced path is intact', { timeout: 90_000 }, async () => {
    const r = await runDirectControl('paced')
    expect(r.gaps, JSON.stringify(r)).toBe(0)
    expect(r.duplicates, JSON.stringify(r)).toBe(0)
    expect(r.played, JSON.stringify(r)).toBeGreaterThanOrEqual(145)
  })

  it('paced 20 ms: no gaps, no duplicates', { timeout: 90_000 }, async () => {
    expectContinuous(await runCase('paced'))
  })

  it('bunched 3 x 20 ms writes every 60 ms', { timeout: 90_000 }, async () => {
    expectContinuous(await runCase('bunched-3x20'))
  })

  it('60 ms Opus packets (one write of 3 frames)', { timeout: 90_000 }, async () => {
    expectContinuous(await runCase('packet-60ms'))
  })

  it('seeded send jitter of +/-30 ms', { timeout: 90_000 }, async () => {
    expectContinuous(await runCase('jitter-30ms'))
  })

  it(
    'after an idle gap a new speaker is heard without gaps or duplicates',
    { timeout: 120_000 },
    async () => {
      // The first case leaves the shared graph's media clock stale; after 1 s idle the pump
      // of the next case's clients resyncs it on its first render, while the new speaker's
      // frames are already queued.
      expectContinuous(await runCase('paced', 'idle-warmup'))
      await delay(1_000)
      expectContinuous(await runCase('paced', 'after-idle'))
    },
  )

  it(
    'after a 1 s pause the same speaker is heard without gaps or duplicates',
    { timeout: 120_000 },
    async () => {
      const [before, after] = await runPausedCase('paced', 'same-speaker-pause', 1_000)
      expectContinuous(before!)
      expectContinuous(after!)
      // A pause is not jitter: the first frame after it waits for the normal prefill (2 frames
      // at 20 ms) plus transport, not the 160 ms an inflated target would add. Both bursts
      // include the same two Opus hops, so compare against the first burst.
      const detail = JSON.stringify({ before: before!.firstFrameMs, after: after!.firstFrameMs })
      expect(after!.stats!.target, detail).toBeLessThanOrEqual(3)
      expect(after!.firstFrameMs, detail).toBeLessThanOrEqual(before!.firstFrameMs + 60)
    },
  )
})
