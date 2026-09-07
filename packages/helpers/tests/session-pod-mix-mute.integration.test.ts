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
      await server.listen(0)
      wsUrl = `ws://localhost:${server.port}`

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

        const sender = pumpSineSources(client1, client3, FRAME_COUNT + 4)
        const [c2Mix, c3Mix] = await Promise.all([
          collectListenerMix(client2, FRAME_COUNT),
          collectListenerMix(client3, FRAME_COUNT),
        ])
        await sender

        assertToneAbsentStereo(c2Mix.left, c2Mix.right, 440, 'muted listener')
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
        host.setTtsPose('client-mix-1', ttsPose)

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
