import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, quatIdentity, vec3Zero } from '@node-webrtc-rust/sdk/mix'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { VoiceSessionBudget, resetProcessVoiceSessionBudget } from '../src/voice-session-budget.js'
import {
  appendStereoChannels,
  assertToneAbsentStereo,
  assertTonePresentStereo,
  defaultIceConfig,
  delay,
  extractChannel,
  goertzelPower,
  waitForConnection,
} from './mix-three-client-helpers.js'
import {
  CLIP_FIXTURE_SPECS,
  clipFixtureDir,
  loadClipFixtures,
  startHoldbackWavServer,
  startE2eClipPlaybackWavServer,
  type ClipEncodingFixture,
} from './clip-fixture-helpers.js'
import {
  accumulateInboundStereoRms,
  assertMuchQuieter,
  stereoRmsFromSplitChannels,
  waitForInboundStereoEnergy,
  waitForInboundStereoQuiet,
} from './mix-energy-helpers.js'
import {
  probeClipPlayInboundEnergy,
  SMOKE_PEER_IDS,
  type SmokeSessionBinding,
} from './smoke-parity-helpers.js'

const SMOKE_SESSION_IDS = ['session-c1', 'session-c2', 'session-c3'] as const

const clipFixtures = loadClipFixtures()
const wavFixture = clipFixtures.find((f) => f.ext === 'wav')

function getClipFixture(ext: string): ClipEncodingFixture | undefined {
  return clipFixtures.find((f) => f.ext === ext)
}

type SessionPodSlot = {
  sessionId: string
  signaling: SignalingClient
  host: VoiceAgentSessionHost
}

type SessionPodTestAccess = SessionPod & {
  slots: Map<string, SessionPodSlot>
}

type HostTestAccess = VoiceAgentSessionHost & {
  sessions: Map<string, { agent?: { sendTextToTTS: (text: string) => Promise<void> } }>
}

function getVoiceHostForSession(
  pod: SessionPod,
  sessionId: string,
): VoiceAgentSessionHost | undefined {
  return (pod as SessionPodTestAccess).slots.get(sessionId)?.host
}

function sessionPodClipNativeAvailable(): boolean {
  if (typeof RTCPeerConnection !== 'function') {
    return false
  }
  try {
    const graph = new AudioMixGraph()
    graph.addInput('probe')
    graph.setSourceMixPlacement('probe', 'left')
    graph.removeInput('probe')
    return true
  } catch {
    return false
  }
}

const QUIET_BASELINE_PROBE_MS = 400
const QUIET_WINDOW_MS = 400
const QUIET_WAIT_MS = 20_000
const STEREO_QUIET_THRESHOLD = 200

const centerPose = { position: vec3Zero(), orientation: quatIdentity() }

function poseAtX(x: number) {
  return {
    position: { ...vec3Zero(), x },
    orientation: quatIdentity(),
  }
}

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

  const signaling = new SignalingClient({
    url: wsUrl,
    room: sessionId,
    peerId,
  })

  const agentAudioPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`timed out waiting for agent audio track (${peerId})`))
    }, 30_000)
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

/** Agent `ontrack` can fire before `agentStarted`; playAudio requires an active voice client. */
async function waitForVoiceClientActive(
  host: VoiceAgentSessionHost,
  peerId: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (host.isVoiceClientActive(peerId)) return
    await delay(20)
  }
  throw new Error(`voice client ${peerId} not active within ${timeoutMs}ms`)
}

async function waitForVoiceClientActiveOnPod(
  pod: SessionPod,
  clientId: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    for (const sessionId of SMOKE_SESSION_IDS) {
      const host = getVoiceHostForSession(pod, sessionId)
      if (host?.isVoiceClientActive(clientId)) return
    }
    await delay(50)
  }
  throw new Error(`timed out waiting for active voice client ${clientId}`)
}

function smokeSessionBindings(pod: SessionPod): SmokeSessionBinding[] {
  return SMOKE_SESSION_IDS.map((sessionId, index) => ({
    peerId: SMOKE_PEER_IDS[index],
    host: getVoiceHostForSession(pod, sessionId)!,
  }))
}

async function connectReadyVoiceClient(
  host: VoiceAgentSessionHost,
  wsUrl: string,
  sessionId: string,
  peerId: string,
): Promise<ConnectedClient> {
  const client = await connectClientToSession(wsUrl, sessionId, peerId)
  await waitForVoiceClientActive(host, peerId)
  return client
}

async function readSampleWithTimeout(
  track: RemoteAudioTrack,
  label: string,
  timeoutMs = 30_000,
): Promise<Buffer> {
  return Promise.race([
    track.readSample(),
    delay(timeoutMs).then(() => {
      throw new Error(`readSample timeout for ${label} after ${timeoutMs}ms`)
    }),
  ])
}

function closeClient(client: ConnectedClient): void {
  client.teardownNegotiate()
  client.pc.close()
  client.signaling.disconnect()
}

async function collectAgentFrames(
  track: RemoteAudioTrack,
  label: string,
  frameCount: number,
): Promise<{ left: Int16Array; right: Int16Array }> {
  const left: number[] = []
  const right: number[] = []
  for (let i = 0; i < frameCount; i++) {
    const pcm = await readSampleWithTimeout(track, label)
    appendStereoChannels(left, right, pcm)
  }
  return { left: Int16Array.from(left), right: Int16Array.from(right) }
}

/** Controller "playing" can precede inbound PCM; wait before tone collect. */
async function waitForInboundThenCollect(
  track: RemoteAudioTrack,
  label: string,
  frameCount: number,
): Promise<{ left: Int16Array; right: Int16Array }> {
  await waitForInboundStereoEnergy(track, {
    threshold: STEREO_QUIET_THRESHOLD,
    timeoutMs: 30_000,
    label: `${label} inbound`,
  })
  return collectAgentFrames(track, label, frameCount)
}

function assertSideDominates(
  left: Int16Array,
  right: Int16Array,
  freqHz: number,
  side: 'left' | 'right',
  label: string,
): void {
  const pL = goertzelPower(left, freqHz)
  const pR = goertzelPower(right, freqHz)
  if (side === 'left' && pL <= 2 * Math.max(pR, 1)) {
    throw new Error(`${label}: expected ${freqHz} Hz on left (L=${pL}, R=${pR})`)
  }
  if (side === 'right' && pR <= 2 * Math.max(pL, 1)) {
    throw new Error(`${label}: expected ${freqHz} Hz on right (L=${pL}, R=${pR})`)
  }
}

async function waitForClipPlaying(
  host: VoiceAgentSessionHost,
  playId: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const status = host.getAudioPlay(playId)
    if (status?.status === 'playing') return
    if (status?.status === 'buffering' && (status.bufferedMs ?? 0) > 0) return
    await delay(50)
  }
  throw new Error(`clip ${playId} did not reach playing within ${timeoutMs}ms`)
}

const CLIP_SESSION_ID = 'clip-integration'

describe.skipIf(!sessionPodClipNativeAvailable())('SessionPod clip playback integration', () => {
  let server: SignalingServer
  let wsUrl: string
  let pod: SessionPod
  const sessionBudget = new VoiceSessionBudget(8)

  beforeAll(async () => {
    resetProcessVoiceSessionBudget()
    server = new SignalingServer({ port: 0 })
    await server.listen(0)
    wsUrl = `ws://localhost:${server.port}`

    pod = new SessionPod(server, {
      signalingUrl: wsUrl,
      iceServers: defaultIceConfig.iceServers,
      sessionMode: 'voice+data',
      teardownIdleSessions: false,
      voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } },
      sessionBudget,
      maxPreparedSessions: 32,
    })
  })

  afterAll(async () => {
    await pod.close().catch(() => undefined)
    resetProcessVoiceSessionBudget()
  })

  for (const spec of CLIP_FIXTURE_SPECS) {
    const fixturePath = join(clipFixtureDir(), spec.file)
    it.skipIf(!existsSync(fixturePath))(
      `plays ${spec.ext} via path and listener hears ${spec.freqHz} Hz`,
      async () => {
        const fixture = getClipFixture(spec.ext)
        expect(fixture).toBeDefined()

        const sessionId = CLIP_SESSION_ID
        const peerId = `client-path-${spec.ext}`
        await pod.ensureSession(sessionId)
        const host = getVoiceHostForSession(pod, sessionId)!
        host.createMixGroup({ id: `solo-${spec.ext}`, clientIds: [peerId] })

        const client = await connectReadyVoiceClient(host, wsUrl, sessionId, peerId)
        try {
          const { playId } = await host.playAudio({ source: { path: fixture!.path } })
          await waitForClipPlaying(host, playId, 30_000)
          // Controller "playing"/"buffering" can precede outbound PCM (MP3 decode +
          // queued mix silence). Collecting a fixed 20 frames then saw L=0,R=0 in CI.
          await waitForInboundStereoEnergy(client.agentAudio, {
            threshold: STEREO_QUIET_THRESHOLD,
            timeoutMs: 30_000,
            label: `${spec.ext} path inbound`,
          })
          const { left, right } = await collectAgentFrames(client.agentAudio, 'clip path', 20)
          assertTonePresentStereo(left, right, spec.freqHz, `${spec.ext} path`)
          host.stopAudioPlay(playId)
        } finally {
          closeClient(client)
          await delay(100)
        }
      },
      120_000,
    )
  }

  it('streaming URL starts playing before server sends tail', async () => {
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.createMixGroup({ id: 'stream', clientIds: ['client-stream'] })

    const holdback = await startHoldbackWavServer(wavFixture!.path)
    const client = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-stream')
    try {
      const { playId } = await host.playAudio({ source: { url: holdback.url } })

      let started = false
      const deadline = Date.now() + 60_000
      while (Date.now() < deadline && !started) {
        const status = host.getAudioPlay(playId)
        if (status?.status === 'playing') {
          started = true
          break
        }
        if (status?.status === 'buffering' && (status.bufferedMs ?? 0) > 0) {
          started = true
          break
        }
        try {
          const pcm = await Promise.race([
            client.agentAudio.readSample(),
            delay(1_000).then(() => null),
          ])
          if (pcm && pcm.length >= 3840) {
            const left = extractChannel(pcm, 0)
            const right = extractChannel(pcm, 1)
            const tonePower = Math.max(
              goertzelPower(left, wavFixture!.freqHz),
              goertzelPower(right, wavFixture!.freqHz),
            )
            if (tonePower > 1_000) {
              started = true
              break
            }
          }
        } catch {
          /* read may race before mix routes clip */
        }
        await delay(100)
      }

      expect(started).toBe(true)
      expect(holdback.tailReleased()).toBe(false)

      holdback.releaseTail()
      await delay(500)
      host.stopAudioPlay(playId)
    } finally {
      holdback.releaseTail()
      await holdback.close()
      closeClient(client)
      await delay(100)
    }
  }, 120_000)

  // One session, two peers, shared MixGraph — same topology as staging clip-playback probe G.
  // SessionPod 3-session pods use isolated mixers and would not reproduce the shared-graph leak.
  it('peerIds targets one client; other client does not hear clip', async () => {
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.createMixGroup({
      id: 'pair',
      clientIds: ['client-a', 'client-b'],
    })

    const clientA = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-a')
    const clientB = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-b')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        peerIds: ['client-a'],
      })
      await waitForClipPlaying(host, playId)
      const heardA = await waitForInboundThenCollect(clientA.agentAudio, 'client-a', 15)
      const heardB = await collectAgentFrames(clientB.agentAudio, 'client-b', 15)
      assertTonePresentStereo(heardA.left, heardA.right, wavFixture!.freqHz, 'target A')
      assertToneAbsentStereo(heardB.left, heardB.right, wavFixture!.freqHz, 'non-target B')

      const targetEnergy = stereoRmsFromSplitChannels(heardA.left, heardA.right)
      const excludedEnergy = stereoRmsFromSplitChannels(heardB.left, heardB.right)
      try {
        assertMuchQuieter(excludedEnergy, targetEnergy)
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        throw new Error(
          `non-target B must be much quieter than target A (shared MixGraph clip leak): ${msg}`,
        )
      }

      host.stopAudioPlay(playId)
    } finally {
      closeClient(clientA)
      closeClient(clientB)
      await delay(100)
    }
  }, 120_000)

  it('clip MixPlacement left pans for listener at origin', async () => {
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(false)
    host.createMixGroup({ id: 'placement-left', clientIds: ['client-pan-left'] })
    host.setClientPose('client-pan-left', centerPose)

    const client = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-pan-left')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { placement: 'left' },
      })
      await waitForClipPlaying(host, playId)
      const mix = await waitForInboundThenCollect(client.agentAudio, 'placement left', 20)
      assertSideDominates(mix.left, mix.right, wavFixture!.freqHz, 'left', 'clip left')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(client)
      await delay(200)
    }
  }, 120_000)

  it('clip MixPlacement right pans for listener at origin', async () => {
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(false)
    host.createMixGroup({ id: 'placement-right', clientIds: ['client-pan-right'] })
    host.setClientPose('client-pan-right', centerPose)

    const client = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-pan-right')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { placement: 'right' },
      })
      await waitForClipPlaying(host, playId)
      const mix = await waitForInboundThenCollect(client.agentAudio, 'placement right', 20)
      assertSideDominates(mix.left, mix.right, wavFixture!.freqHz, 'right', 'clip right')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(client)
      await delay(200)
    }
  }, 120_000)

  it('clip world pose +x pans right with positional mixing on', async () => {
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(true)
    host.createMixGroup({ id: 'pose-pos', clientIds: ['client-pose-pos'] })
    host.setClientPose('client-pose-pos', centerPose)

    const client = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-pose-pos')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { pose: poseAtX(3) },
      })
      await waitForClipPlaying(host, playId)
      const mix = await waitForInboundThenCollect(client.agentAudio, 'pose +x', 20)
      assertSideDominates(mix.left, mix.right, wavFixture!.freqHz, 'right', 'pose +x')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(client)
      await delay(200)
    }
  }, 120_000)

  it('clip world pose -x pans left with positional mixing on', async () => {
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(true)
    host.createMixGroup({ id: 'pose-neg', clientIds: ['client-pose-neg'] })
    host.setClientPose('client-pose-neg', centerPose)

    const client = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-pose-neg')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { pose: poseAtX(-3) },
      })
      await waitForClipPlaying(host, playId)
      const mix = await waitForInboundThenCollect(client.agentAudio, 'pose -x', 20)
      assertSideDominates(mix.left, mix.right, wavFixture!.freqHz, 'left', 'pose -x')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(client)
      await delay(200)
    }
  }, 120_000)

  it('setTtsPosition placement then speak pans mock TTS right', async () => {
    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId) as HostTestAccess
    host.setPositionalMixing(false)
    host.createMixGroup({ id: 'tts-placement', clientIds: ['client-tts'] })
    host.setClientPose('client-tts', centerPose)
    host.setTtsPosition({ placement: 'right' })

    const client = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-tts')
    try {
      const agent = host.sessions.get('client-tts')?.agent
      expect(agent).toBeDefined()
      await agent!.sendTextToTTS('hello')
      const mix = await collectAgentFrames(client.agentAudio, 'tts placement', 25)
      const mockTtsFreq = 440 + 'mock'.length * 10
      assertSideDominates(mix.left, mix.right, mockTtsFreq, 'right', 'tts placement right')
    } finally {
      closeClient(client)
      await delay(100)
    }
  }, 120_000)

  it('clip-playback-smoke parity: probe G then overlapping H (runner per-session playAudio)', async () => {
    expect(wavFixture).toBeDefined()

    for (const sessionId of SMOKE_SESSION_IDS) {
      await pod.ensureSession(sessionId)
    }

    const [client1, client2, client3] = await Promise.all([
      connectClientToSession(wsUrl, SMOKE_SESSION_IDS[0], SMOKE_PEER_IDS[0]),
      connectClientToSession(wsUrl, SMOKE_SESSION_IDS[1], SMOKE_PEER_IDS[1]),
      connectClientToSession(wsUrl, SMOKE_SESSION_IDS[2], SMOKE_PEER_IDS[2]),
    ])

    try {
      await client1.mic.writeSample(Buffer.alloc(960), 5)
      await client2.mic.writeSample(Buffer.alloc(960), 5)
      await client3.mic.writeSample(Buffer.alloc(960), 5)

      for (const peerId of SMOKE_PEER_IDS) {
        await waitForVoiceClientActiveOnPod(pod, peerId)
      }

      const driverHost = getVoiceHostForSession(pod, SMOKE_SESSION_IDS[0])!
      driverHost.createMixGroup({ id: 'clip-shared', clientIds: [...SMOKE_PEER_IDS] })

      const bindings = smokeSessionBindings(pod)
      const listener = client2
      const clipSource = { path: wavFixture!.path }

      await waitForInboundStereoQuiet(listener.agentAudio, {
        threshold: STEREO_QUIET_THRESHOLD,
        quietWindowMs: QUIET_WINDOW_MS,
        timeoutMs: QUIET_WAIT_MS,
        label: 'quiet baseline before clip play',
      })
      const quietBaseline = await accumulateInboundStereoRms(
        listener.agentAudio,
        QUIET_BASELINE_PROBE_MS,
      )

      const [energyGListener, energyGExcluded] = await probeClipPlayInboundEnergy(
        bindings,
        { peerIds: [SMOKE_PEER_IDS[1]] },
        [listener.agentAudio, client3.agentAudio],
        clipSource,
      )
      assertMuchQuieter(quietBaseline, energyGListener)
      assertMuchQuieter(energyGExcluded, energyGListener)

      const hClients = [client1, client2, client3]
      const energiesH = await probeClipPlayInboundEnergy(
        bindings,
        {},
        hClients.map((client) => client.agentAudio),
        clipSource,
      )
      for (const [index, energy] of energiesH.entries()) {
        try {
          assertMuchQuieter(quietBaseline, energy)
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err)
          throw new Error(
            `${SMOKE_PEER_IDS[index]} must hear probe H while G may still be playing: ${msg} (left=${energy.left.toFixed(1)} right=${energy.right.toFixed(1)})`,
          )
        }
        if (index === 0) {
          expect(Math.max(energy.left, energy.right)).toBeGreaterThan(STEREO_QUIET_THRESHOLD)
        }
      }
    } finally {
      closeClient(client1)
      closeClient(client2)
      closeClient(client3)
      await delay(100)
    }
  }, 180_000)

  it('clip-playback-smoke parity: probe G then overlapping H via local URL', async () => {
    const clipServer = await startE2eClipPlaybackWavServer()

    for (const sessionId of SMOKE_SESSION_IDS) {
      await pod.ensureSession(sessionId)
    }

    const [client1, client2, client3] = await Promise.all([
      connectClientToSession(wsUrl, SMOKE_SESSION_IDS[0], SMOKE_PEER_IDS[0]),
      connectClientToSession(wsUrl, SMOKE_SESSION_IDS[1], SMOKE_PEER_IDS[1]),
      connectClientToSession(wsUrl, SMOKE_SESSION_IDS[2], SMOKE_PEER_IDS[2]),
    ])

    try {
      await client1.mic.writeSample(Buffer.alloc(960), 5)
      await client2.mic.writeSample(Buffer.alloc(960), 5)
      await client3.mic.writeSample(Buffer.alloc(960), 5)

      for (const peerId of SMOKE_PEER_IDS) {
        await waitForVoiceClientActiveOnPod(pod, peerId)
      }

      const driverHost = getVoiceHostForSession(pod, SMOKE_SESSION_IDS[0])!
      driverHost.createMixGroup({ id: 'clip-url-shared', clientIds: [...SMOKE_PEER_IDS] })

      const bindings = smokeSessionBindings(pod)
      const listener = client2
      const clipSource = { url: clipServer.url }

      await waitForInboundStereoQuiet(listener.agentAudio, {
        threshold: STEREO_QUIET_THRESHOLD,
        quietWindowMs: QUIET_WINDOW_MS,
        timeoutMs: QUIET_WAIT_MS,
        label: 'quiet baseline before URL clip play',
      })
      const quietBaseline = await accumulateInboundStereoRms(
        listener.agentAudio,
        QUIET_BASELINE_PROBE_MS,
      )

      const [energyGListener, energyGExcluded] = await probeClipPlayInboundEnergy(
        bindings,
        { peerIds: [SMOKE_PEER_IDS[1]] },
        [listener.agentAudio, client3.agentAudio],
        clipSource,
      )
      assertMuchQuieter(quietBaseline, energyGListener)
      assertMuchQuieter(energyGExcluded, energyGListener)

      const hClients = [client1, client2, client3]
      const energiesH = await probeClipPlayInboundEnergy(
        bindings,
        {},
        hClients.map((client) => client.agentAudio),
        clipSource,
      )
      for (const [index, energy] of energiesH.entries()) {
        try {
          assertMuchQuieter(quietBaseline, energy)
        } catch (err) {
          const msg = err instanceof Error ? err.message : String(err)
          throw new Error(
            `${SMOKE_PEER_IDS[index]} must hear URL probe H while G may still be playing: ${msg} (left=${energy.left.toFixed(1)} right=${energy.right.toFixed(1)})`,
          )
        }
        if (index === 0) {
          expect(Math.max(energy.left, energy.right)).toBeGreaterThan(STEREO_QUIET_THRESHOLD)
        }
      }
    } finally {
      await clipServer.close()
      closeClient(client1)
      closeClient(client2)
      closeClient(client3)
      await delay(100)
    }
  }, 180_000)

  it('setTtsPosition pose pans mock TTS when positional mixing on', async () => {
    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId) as HostTestAccess
    host.setPositionalMixing(true)
    host.createMixGroup({ id: 'tts-pose', clientIds: ['client-tts-pose'] })
    host.setClientPose('client-tts-pose', centerPose)
    await host.setTtsPosition({ pose: poseAtX(3) }, { clientId: 'client-tts-pose' })

    const client = await connectReadyVoiceClient(host, wsUrl, sessionId, 'client-tts-pose')
    try {
      const agent = host.sessions.get('client-tts-pose')?.agent
      expect(agent).toBeDefined()
      await agent!.sendTextToTTS('hi')
      const mix = await collectAgentFrames(client.agentAudio, 'tts pose', 25)
      const mockTtsFreq = 440 + 'mock'.length * 10
      assertSideDominates(mix.left, mix.right, mockTtsFreq, 'right', 'tts pose +x')
    } finally {
      closeClient(client)
      await delay(100)
    }
  }, 120_000)
})
