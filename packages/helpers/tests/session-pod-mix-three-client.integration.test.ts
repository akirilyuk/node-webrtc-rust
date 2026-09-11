import { describe, expect, it } from 'vitest'

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, quatIdentity, vec3Zero } from '@node-webrtc-rust/sdk/mix'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { VoiceSessionBudget, resetProcessVoiceSessionBudget } from '../src/voice-session-budget.js'
import {
  appendStereoChannels,
  assertTwoSinePanSides,
  defaultIceConfig,
  delay,
  FRAME_COUNT,
  sineStereoFrame,
  SAMPLES_PER_CHANNEL,
  waitForClosed,
  waitForConnection,
} from './mix-three-client-helpers.js'

// Same-process loopback: do not set WEBRTC_NAT_1TO1_IPS (breaks ICE in CI).

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
const SESSION_ID_PREFIXES = ['session-c1', 'session-c2', 'session-c3'] as const

const centerPose = { position: vec3Zero(), orientation: quatIdentity() }

function poseAtX(x: number) {
  return {
    position: { ...vec3Zero(), x },
    orientation: quatIdentity(),
  }
}

function sessionIdsForRun(runId: string): readonly [string, string, string] {
  return [
    `${SESSION_ID_PREFIXES[0]}-${runId}`,
    `${SESSION_ID_PREFIXES[1]}-${runId}`,
    `${SESSION_ID_PREFIXES[2]}-${runId}`,
  ]
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

async function closeClient(client: ConnectedClient): Promise<void> {
  client.teardownNegotiate()
  client.mic.stop()
  client.agentAudio.stop()
  if (typeof client.pc.closeAsync === 'function') {
    await client.pc.closeAsync()
  } else {
    client.pc.close()
  }
  await waitForClosed(client.pc)
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

async function runThreeClientPositionalMix(
  c1X: number,
  c3X: number,
  expect440OnRight: boolean,
): Promise<void> {
  resetProcessVoiceSessionBudget()
  const runId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
  const sessionIds = sessionIdsForRun(runId)

  const server = new SignalingServer({ port: 0 })
  await server.listen(0)
  const wsUrl = `ws://localhost:${server.port}`
  const sessionBudget = new VoiceSessionBudget(8)

  const pod = new SessionPod(server, {
    signalingUrl: wsUrl,
    iceServers: defaultIceConfig.iceServers,
    sessionMode: 'voice+data',
    teardownIdleSessions: false,
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } },
    sessionBudget,
    maxPreparedSessions: 8,
  })

  try {
    for (const sessionId of sessionIds) {
      await pod.ensureSession(sessionId)
    }

    const [client1, client2, client3] = await Promise.all([
      connectClientToSession(wsUrl, sessionIds[0], 'client-mix-1'),
      connectClientToSession(wsUrl, sessionIds[1], 'client-mix-2'),
      connectClientToSession(wsUrl, sessionIds[2], 'client-mix-3'),
    ])

    try {
      const host = getVoiceHostForSession(pod, sessionIds[1])
      expect(host).toBeDefined()
      configurePositionalMix(host!, c1X, c3X)

      await client1.mic.writeSample(Buffer.alloc(960), 5)
      await client3.mic.writeSample(Buffer.alloc(960), 5)
      await client2.mic.writeSample(Buffer.alloc(960), 5)

      const phaseRefC1 = { value: 0 }
      const phaseRefC3 = { value: 0 }

      const senders = (async () => {
        for (let i = 0; i < FRAME_COUNT + 4; i++) {
          await client1.mic.writeSample(sineStereoFrame(440, 10_000, phaseRefC1.value), 20)
          await client3.mic.writeSample(sineStereoFrame(880, 10_000, phaseRefC3.value), 20)
          phaseRefC1.value += SAMPLES_PER_CHANNEL
          phaseRefC3.value += SAMPLES_PER_CHANNEL
        }
      })()

      const left: number[] = []
      const right: number[] = []

      for (let i = 0; i < FRAME_COUNT; i++) {
        const pcm2 = await readSampleWithTimeout(client2.agentAudio, 'client-mix-2 agent mix')
        appendStereoChannels(left, right, pcm2)
      }

      await senders

      assertTwoSinePanSides(Int16Array.from(left), Int16Array.from(right), expect440OnRight)
    } finally {
      await closeClient(client1)
      await closeClient(client2)
      await closeClient(client3)
    }
  } finally {
    // SessionPod.close() tears down slots and closes the shared SignalingServer.
    await pod.close().catch(() => undefined)
    resetProcessVoiceSessionBudget()
  }
}

describe.skipIf(!sessionPodMixIntegrationNativeAvailable())(
  'SessionPod three-client positional mix integration',
  () => {
    it('listener hears 440 Hz right / 880 Hz left via helpers mix APIs', async () => {
      await runThreeClientPositionalMix(3, -3, true)
    }, 120_000)

    it('swapped source poses flip stereo sides', async () => {
      await runThreeClientPositionalMix(-3, 3, false)
    }, 120_000)
  },
)
