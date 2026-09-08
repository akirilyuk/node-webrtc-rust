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
  canEncodeClip,
  generateClipFixtures,
  startHoldbackWavServer,
} from './clip-fixture-helpers.js'

const ENCODING_SPECS = [
  { ext: 'wav', freqHz: 440 },
  { ext: 'mp3', freqHz: 523 },
  { ext: 'flac', freqHz: 659 },
  { ext: 'ogg', freqHz: 784 },
  { ext: 'aac', freqHz: 880 },
  { ext: 'm4a', freqHz: 988 },
  { ext: 'pcm', freqHz: 1_100 },
]

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
    generateClipFixtures()
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

  for (const spec of ENCODING_SPECS) {
    it.skipIf(!canEncodeClip(spec.ext))(
      `plays ${spec.ext} via path and listener hears ${spec.freqHz} Hz`,
      async () => {
        const fixture = generateClipFixtures().find((f) => f.ext === spec.ext)
        expect(fixture).toBeDefined()

        const sessionId = CLIP_SESSION_ID
        const peerId = `client-path-${spec.ext}`
        await pod.ensureSession(sessionId)
        const host = getVoiceHostForSession(pod, sessionId)!
        host.createMixGroup({ id: `solo-${spec.ext}`, clientIds: [peerId] })

        const client = await connectClientToSession(wsUrl, sessionId, peerId)
        try {
          const { playId } = await host.playAudio({ source: { path: fixture!.path } })
          await waitForClipPlaying(host, playId, 30_000)
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
    const wavFixture = generateClipFixtures().find((f) => f.ext === 'wav')
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.createMixGroup({ id: 'stream', clientIds: ['client-stream'] })

    const holdback = await startHoldbackWavServer(wavFixture!.path)
    const client = await connectClientToSession(wsUrl, sessionId, 'client-stream')
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

  it('peerIds targets one client; other client does not hear clip', async () => {
    const wavFixture = generateClipFixtures().find((f) => f.ext === 'wav')
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.createMixGroup({
      id: 'pair',
      clientIds: ['client-a', 'client-b'],
    })

    const clientA = await connectClientToSession(wsUrl, sessionId, 'client-a')
    const clientB = await connectClientToSession(wsUrl, sessionId, 'client-b')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        peerIds: ['client-a'],
      })
      await waitForClipPlaying(host, playId)
      const heardA = await collectAgentFrames(clientA.agentAudio, 'client-a', 15)
      const heardB = await collectAgentFrames(clientB.agentAudio, 'client-b', 15)
      assertTonePresentStereo(heardA.left, heardA.right, wavFixture!.freqHz, 'target A')
      assertToneAbsentStereo(heardB.left, heardB.right, wavFixture!.freqHz, 'non-target B')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(clientA)
      closeClient(clientB)
      await delay(100)
    }
  }, 120_000)

  it('clip MixPlacement left pans for listener at origin', async () => {
    const wavFixture = generateClipFixtures().find((f) => f.ext === 'wav')
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(false)
    host.createMixGroup({ id: 'placement-left', clientIds: ['client-pan-left'] })
    host.setClientPose('client-pan-left', centerPose)

    const client = await connectClientToSession(wsUrl, sessionId, 'client-pan-left')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { placement: 'left' },
      })
      await waitForClipPlaying(host, playId)
      const mix = await collectAgentFrames(client.agentAudio, 'placement left', 20)
      assertSideDominates(mix.left, mix.right, wavFixture!.freqHz, 'left', 'clip left')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(client)
      await delay(200)
    }
  }, 120_000)

  it('clip MixPlacement right pans for listener at origin', async () => {
    const wavFixture = generateClipFixtures().find((f) => f.ext === 'wav')
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(false)
    host.createMixGroup({ id: 'placement-right', clientIds: ['client-pan-right'] })
    host.setClientPose('client-pan-right', centerPose)

    const client = await connectClientToSession(wsUrl, sessionId, 'client-pan-right')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { placement: 'right' },
      })
      await waitForClipPlaying(host, playId)
      const mix = await collectAgentFrames(client.agentAudio, 'placement right', 20)
      assertSideDominates(mix.left, mix.right, wavFixture!.freqHz, 'right', 'clip right')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(client)
      await delay(200)
    }
  }, 120_000)

  it('clip world pose +x pans right with positional mixing on', async () => {
    const wavFixture = generateClipFixtures().find((f) => f.ext === 'wav')
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(true)
    host.createMixGroup({ id: 'pose-pos', clientIds: ['client-pose-pos'] })
    host.setClientPose('client-pose-pos', centerPose)

    const client = await connectClientToSession(wsUrl, sessionId, 'client-pose-pos')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { pose: poseAtX(3) },
      })
      await waitForClipPlaying(host, playId)
      const mix = await collectAgentFrames(client.agentAudio, 'pose +x', 20)
      assertSideDominates(mix.left, mix.right, wavFixture!.freqHz, 'right', 'pose +x')
      host.stopAudioPlay(playId)
    } finally {
      closeClient(client)
      await delay(200)
    }
  }, 120_000)

  it('clip world pose -x pans left with positional mixing on', async () => {
    const wavFixture = generateClipFixtures().find((f) => f.ext === 'wav')
    expect(wavFixture).toBeDefined()

    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId)!
    host.setPositionalMixing(true)
    host.createMixGroup({ id: 'pose-neg', clientIds: ['client-pose-neg'] })
    host.setClientPose('client-pose-neg', centerPose)

    const client = await connectClientToSession(wsUrl, sessionId, 'client-pose-neg')
    try {
      const { playId } = await host.playAudio({
        source: { path: wavFixture!.path },
        position: { pose: poseAtX(-3) },
      })
      await waitForClipPlaying(host, playId)
      const mix = await collectAgentFrames(client.agentAudio, 'pose -x', 20)
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

    const client = await connectClientToSession(wsUrl, sessionId, 'client-tts')
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

  it('setTtsPosition pose pans mock TTS when positional mixing on', async () => {
    const sessionId = CLIP_SESSION_ID
    await pod.ensureSession(sessionId)
    const host = getVoiceHostForSession(pod, sessionId) as HostTestAccess
    host.setPositionalMixing(true)
    host.createMixGroup({ id: 'tts-pose', clientIds: ['client-tts-pose'] })
    host.setClientPose('client-tts-pose', centerPose)
    host.setTtsPosition({ pose: poseAtX(3) }, { clientId: 'client-tts-pose' })

    const client = await connectClientToSession(wsUrl, sessionId, 'client-tts-pose')
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
