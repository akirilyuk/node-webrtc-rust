import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, quatIdentity, vec3Zero } from '@node-webrtc-rust/sdk/mix'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import type { ClientAudioMixer } from '../src/client-audio-mixer.js'
import { SessionPod } from '../src/session-pod.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { VoiceSessionBudget, resetProcessVoiceSessionBudget } from '../src/voice-session-budget.js'
import {
  appendStereoChannels,
  assertToneAbsentStereo,
  assertTonePresentStereo,
  defaultIceConfig,
  delay,
  FRAME_COUNT,
  goertzelPower,
  sineStereoFrame,
  SAMPLES_PER_CHANNEL,
  waitForConnection,
} from './mix-three-client-helpers.js'

type SessionPodSlot = {
  sessionId: string
  signaling: SignalingClient
  host: VoiceAgentSessionHost
}

type SessionPodTestAccess = SessionPod & {
  slots: Map<string, SessionPodSlot>
}

function getVoiceHostForSession(
  pod: SessionPod,
  sessionId: string,
): VoiceAgentSessionHost | undefined {
  return (pod as SessionPodTestAccess).slots.get(sessionId)?.host
}

function sessionPodMixIntegrationNativeAvailable(): boolean {
  if (typeof RTCPeerConnection !== 'function') {
    return false
  }
  try {
    const graph = new AudioMixGraph()
    graph.addInput('probe')
    graph.removeInput('probe')
    return true
  } catch {
    return false
  }
}

/** Max 440 Hz power of a muted listener relative to a listener that hears the source (-30 dB). */
const MUTED_LEAK_MAX_RATIO = 1e-3

const CLIENT_IDS = ['client-mix-1', 'client-mix-2', 'client-mix-3'] as const
const SESSION_IDS = ['session-c1', 'session-c2', 'session-c3'] as const

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

function configurePositionalMix(host: VoiceAgentSessionHost, c1X: number, c3X: number): void {
  host.createMixGroup({
    id: 'all',
    clientIds: [...CLIENT_IDS],
  })
  host.setPositionalMixing(true)
  host.setClientPose('client-mix-2', centerPose)
  host.setClientPose('client-mix-1', poseAtX(c1X))
  host.setClientPose('client-mix-3', poseAtX(c3X))
}

async function pumpSineSources(
  client1: ConnectedClient,
  client3: ConnectedClient,
  frames: number,
): Promise<void> {
  const phaseRefC1 = { value: 0 }
  const phaseRefC3 = { value: 0 }
  for (let i = 0; i < frames; i++) {
    await client1.mic.writeSample(sineStereoFrame(440, 10_000, phaseRefC1.value), 20)
    await client3.mic.writeSample(sineStereoFrame(880, 10_000, phaseRefC3.value), 20)
    phaseRefC1.value += SAMPLES_PER_CHANNEL
    phaseRefC3.value += SAMPLES_PER_CHANNEL
  }
}

async function collectListenerMix(
  listener: ConnectedClient,
  frames: number,
): Promise<{ left: Int16Array; right: Int16Array }> {
  const left: number[] = []
  const right: number[] = []
  for (let i = 0; i < frames; i++) {
    const pcm = await readSampleWithTimeout(listener.agentAudio, 'listener mix')
    appendStereoChannels(left, right, pcm)
  }
  return { left: Int16Array.from(left), right: Int16Array.from(right) }
}

/**
 * Writes both sources at real-time pace (one 20 ms frame per 20 ms) until `shouldStop()`.
 * An unpaced burst reaches the listener as ~50 ms of audio that can land entirely outside a
 * fixed read window, because the listener's read queue drains idle frames faster than real time.
 */
async function pumpSineSourcesUntil(
  client1: ConnectedClient,
  client3: ConnectedClient,
  shouldStop: () => boolean,
): Promise<void> {
  const phaseC1 = { value: 0 }
  const phaseC3 = { value: 0 }
  let nextAt = Date.now()
  while (!shouldStop()) {
    await client1.mic.writeSample(sineStereoFrame(440, 10_000, phaseC1.value), 20)
    await client3.mic.writeSample(sineStereoFrame(880, 10_000, phaseC3.value), 20)
    phaseC1.value += SAMPLES_PER_CHANNEL
    phaseC3.value += SAMPLES_PER_CHANNEL
    nextAt += 20
    await delay(Math.max(0, nextAt - Date.now()))
  }
}

/**
 * Share of the window's energy carried by `freqHz`: 1 for a pure tone filling the window,
 * about 0 for silence or noise. Independent of mix gain and panning, so it tells a window
 * that holds the whole tone from one that caught only the start or end of it.
 */
function toneEnergyFraction(samples: Int16Array, freqHz: number): number {
  let energy = 0
  for (const x of samples) energy += x * x
  if (energy === 0) return 0
  return goertzelPower(samples, freqHz) / ((samples.length / 2) * energy)
}

const FULL_TONE_MIN_FRACTION = 0.5

function fullToneStereo(left: Int16Array, right: Int16Array, freqHz: number): boolean {
  return (
    Math.max(toneEnergyFraction(left, freqHz), toneEnergyFraction(right, freqHz)) >=
    FULL_TONE_MIN_FRACTION
  )
}

/**
 * Reads frames until a sliding window of FRAME_COUNT frames is dominated by `freqHz` (a full window of the tone), so the
 * assertion starts after audio actually flows instead of at an arbitrary time. Every frame
 * read is also passed to `onFrame`.
 */
async function waitForToneWindow(
  listener: ConnectedClient,
  freqHz: number,
  label: string,
  onFrame: (pcm: Buffer) => void,
  timeoutMs = 20_000,
): Promise<{ left: Int16Array; right: Int16Array }> {
  const deadline = Date.now() + timeoutMs
  const windowFrames: Buffer[] = []
  while (Date.now() < deadline) {
    const pcm = await readSampleWithTimeout(listener.agentAudio, label)
    onFrame(pcm)
    windowFrames.push(pcm)
    if (windowFrames.length > FRAME_COUNT) windowFrames.shift()
    if (windowFrames.length < FRAME_COUNT) continue
    const left: number[] = []
    const right: number[] = []
    for (const frame of windowFrames) appendStereoChannels(left, right, frame)
    const l = Int16Array.from(left)
    const r = Int16Array.from(right)
    if (fullToneStereo(l, r, freqHz)) return { left: l, right: r }
  }
  throw new Error(
    `${label}: no full ${freqHz} Hz tone in any ${FRAME_COUNT}-frame window after ${timeoutMs}ms`,
  )
}

type VoiceHostTestAccess = VoiceAgentSessionHost & {
  getClientMixer(): ClientAudioMixer | undefined
}

function getSharedMixGraphFromPod(pod: SessionPod): AudioMixGraph {
  const hosts = SESSION_IDS.map((sessionId) => getVoiceHostForSession(pod, sessionId)).filter(
    (host): host is VoiceAgentSessionHost => host != null,
  )
  expect(hosts.length).toBe(3)
  const graphs = hosts.map((host) => (host as VoiceHostTestAccess).getClientMixer()?.getMixGraph())
  expect(graphs[0]).toBeDefined()
  expect(graphs[1]).toBe(graphs[0])
  expect(graphs[2]).toBe(graphs[0])
  return graphs[0] as AudioMixGraph
}

function assertGraphRenderSkipsMutedSource(
  graph: AudioMixGraph,
  mutedId: string,
  listenerId: string,
  label: string,
): void {
  expect(graph.isGloballyMuted(mutedId)).toBe(true)
  const pcm = graph.renderOutput(listenerId)
  const left: number[] = []
  const right: number[] = []
  appendStereoChannels(left, right, pcm)
  assertToneAbsentStereo(Int16Array.from(left), Int16Array.from(right), 440, label)
}

async function waitForVoiceClientActive(
  pod: SessionPod,
  clientId: string,
  timeoutMs = 15_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    for (const sessionId of SESSION_IDS) {
      const host = getVoiceHostForSession(pod, sessionId)
      if (host?.isVoiceClientActive(clientId)) {
        return
      }
    }
    await delay(50)
  }
  throw new Error(`timed out waiting for active voice client ${clientId}`)
}

async function resetMixMuteState(pod: SessionPod): Promise<void> {
  for (const clientId of CLIENT_IDS) {
    try {
      await pod.setGlobalMute(clientId, false)
    } catch {
      /* session or graph may be torn down */
    }
  }
  for (const listenerId of CLIENT_IDS) {
    for (const targetId of CLIENT_IDS) {
      if (listenerId === targetId) continue
      try {
        await pod.setListenerMute(listenerId, targetId, false)
      } catch {
        /* clients may not share a group */
      }
    }
  }
}

async function setupThreeClientMix(pod: SessionPod, wsUrl: string) {
  for (const sessionId of SESSION_IDS) {
    await pod.ensureSession(sessionId)
  }

  const [client1, client2, client3] = await Promise.all([
    connectClientToSession(wsUrl, 'session-c1', 'client-mix-1'),
    connectClientToSession(wsUrl, 'session-c2', 'client-mix-2'),
    connectClientToSession(wsUrl, 'session-c3', 'client-mix-3'),
  ])

  const host = getVoiceHostForSession(pod, 'session-c2')
  expect(host).toBeDefined()
  configurePositionalMix(host!, 3, -3)

  await client1.mic.writeSample(Buffer.alloc(960), 5)
  await client3.mic.writeSample(Buffer.alloc(960), 5)
  await client2.mic.writeSample(Buffer.alloc(960), 5)

  for (const clientId of CLIENT_IDS) {
    await waitForVoiceClientActive(pod, clientId)
  }

  return { client1, client2, client3 }
}

describe.skipIf(!sessionPodMixIntegrationNativeAvailable())(
  'SessionPod mix mute integration',
  () => {
    let server: SignalingServer
    let wsUrl: string
    let pod: SessionPod
    const sessionBudget = new VoiceSessionBudget(8)

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
        sessionBudget,
        maxPreparedSessions: 8,
      })
    })

    afterAll(async () => {
      await pod.close().catch(() => undefined)
      resetProcessVoiceSessionBudget()
    })

    afterEach(async () => {
      await resetMixMuteState(pod)
    })

    it(
      'global mute silences mix output and disables STT while keeping group membership',
      { timeout: 120_000 },
      async () => {
        const { client1, client2, client3 } = await setupThreeClientMix(pod, wsUrl)
        try {
          await pod.setGlobalMute('client-mix-1', true)

          const graph = getSharedMixGraphFromPod(pod)
          assertGraphRenderSkipsMutedSource(
            graph,
            'client-mix-1',
            'client-mix-2',
            'native graph after global mute',
          )

          const sender = pumpSineSources(client1, client3, FRAME_COUNT + 4)
          const { left, right } = await collectListenerMix(client2, FRAME_COUNT)
          await sender

          assertToneAbsentStereo(left, right, 440, 'listener after global mute')

          const status = pod.getClientMixStatus('client-mix-1')
          expect(status.globallyMuted).toBe(true)
          expect(status.sttEnabled).toBe(false)
          expect(status.groupId).toBe('all')
        } finally {
          closeClient(client1)
          closeClient(client2)
          closeClient(client3)
          await delay(100)
        }
      },
      120_000,
    )

    it('explicit STT stays enabled while globally muted', async () => {
      const { client1, client2, client3 } = await setupThreeClientMix(pod, wsUrl)
      try {
        await pod.setGlobalMute('client-mix-1', true, { sttEnabled: true })

        const graph = getSharedMixGraphFromPod(pod)
        assertGraphRenderSkipsMutedSource(
          graph,
          'client-mix-1',
          'client-mix-2',
          'native graph with STT override',
        )

        const status = pod.getClientMixStatus('client-mix-1')
        expect(status.globallyMuted).toBe(true)
        expect(status.sttEnabled).toBe(true)
      } finally {
        closeClient(client1)
        closeClient(client2)
        closeClient(client3)
        await delay(100)
      }
    }, 120_000)

    it('listener mute silences one listener while others still hear the source', async () => {
      const { client1, client2, client3 } = await setupThreeClientMix(pod, wsUrl)
      try {
        await pod.setListenerMute('client-mix-2', 'client-mix-1', true)

        let stop = false
        const sender = pumpSineSourcesUntil(client1, client3, () => stop)
        // Keep draining the muted listener while the other listener waits for the tone, so the
        // muted listener's window covers the same period in which the source was audible.
        const mutedFrames: Buffer[] = []
        const mutedDrain = (async () => {
          while (!stop) {
            mutedFrames.push(await readSampleWithTimeout(client2.agentAudio, 'muted listener'))
            if (mutedFrames.length > FRAME_COUNT) mutedFrames.shift()
          }
        })()
        let c3Mix: { left: Int16Array; right: Int16Array }
        try {
          c3Mix = await waitForToneWindow(client3, 440, 'other listener', () => undefined)
          const mutedDeadline = Date.now() + 20_000
          while (mutedFrames.length < FRAME_COUNT && Date.now() < mutedDeadline) {
            await delay(20)
          }
        } finally {
          stop = true
          await Promise.all([sender, mutedDrain])
        }

        const c2Left: number[] = []
        const c2Right: number[] = []
        expect(mutedFrames.length).toBe(FRAME_COUNT)
        for (const frame of mutedFrames) appendStereoChannels(c2Left, c2Right, frame)
        // The muted listener still hears the other source (880 Hz), so a fixed noise-floor
        // ratio is too brittle: a clean 880 Hz stream leaks a little into 440 Hz. Compare to
        // the level the other listener hears for the same source instead; a real leak would be
        // within a few dB of it, codec/mix leakage is tens of dB below.
        const mutedLeft = Int16Array.from(c2Left)
        const mutedRight = Int16Array.from(c2Right)
        const mutedPower = Math.max(goertzelPower(mutedLeft, 440), goertzelPower(mutedRight, 440))
        const audiblePower = Math.max(
          goertzelPower(c3Mix.left, 440),
          goertzelPower(c3Mix.right, 440),
        )
        expect(
          mutedPower,
          `muted listener 440 Hz power ${mutedPower} vs audible ${audiblePower}`,
        ).toBeLessThan(audiblePower * MUTED_LEAK_MAX_RATIO)
        assertTonePresentStereo(c3Mix.left, c3Mix.right, 440, 'other listener')

        const status = pod.getClientMixStatus('client-mix-1')
        expect(status.sttEnabled).toBe(true)
        expect(status.mutedBy).toContain('client-mix-2')
        expect(status.mutedBy).not.toContain('client-mix-3')
      } finally {
        closeClient(client1)
        closeClient(client2)
        closeClient(client3)
        await delay(100)
      }
    }, 120_000)

    it('status snapshot reports client and TTS poses', async () => {
      const { client1, client2, client3 } = await setupThreeClientMix(pod, wsUrl)
      try {
        const host = getVoiceHostForSession(pod, 'session-c2')!
        const clientPose = poseAtX(5)
        const ttsPose = poseAtX(-5)
        host.setClientPose('client-mix-1', clientPose)
        await host.setTtsPose('client-mix-1', ttsPose)

        const status = pod.getClientMixStatus('client-mix-1')
        expect(status.pose?.position.x).toBe(5)
        expect(status.ttsPose?.position.x).toBe(-5)
      } finally {
        closeClient(client1)
        closeClient(client2)
        closeClient(client3)
        await delay(100)
      }
    }, 120_000)

    it('listener mute throws when clients are not in the same mix group', async () => {
      const { client1, client2, client3 } = await setupThreeClientMix(pod, wsUrl)
      try {
        const host = getVoiceHostForSession(pod, 'session-c2')!
        host.removeClientFromMix('all', 'client-mix-3')
        await expect(pod.setListenerMute('client-mix-2', 'client-mix-3', true)).rejects.toThrow(
          /same mix group/,
        )
      } finally {
        closeClient(client1)
        closeClient(client2)
        closeClient(client3)
        await delay(100)
      }
    }, 120_000)
  },
)
