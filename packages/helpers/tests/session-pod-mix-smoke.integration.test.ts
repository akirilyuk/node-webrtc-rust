/**
 * SessionPod mix-smoke integration — mirrors staging e2e voice-data-mix-smoke
 * energy probes A–F (exclusive loud mic L/R pan, global/listener mute, TTS pan via speak).
 * Uses real RTCPeerConnections + native AudioMixGraph + mock TTS (dual-mono sine).
 *
 * Verify:
 *   cd node-webrtc-rust
 *   npx vitest run packages/helpers/tests/session-pod-mix-smoke.integration.test.ts
 */
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, quatIdentity, vec3Zero } from '@node-webrtc-rust/sdk/mix'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { SessionPod } from '../src/session-pod.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { VoiceSessionBudget, resetProcessVoiceSessionBudget } from '../src/voice-session-budget.js'
import {
  accumulateInboundStereoRms,
  assertLeftLouder,
  assertMuchQuieter,
  assertRightLouder,
  pumpLoudMicFrames,
  waitForInboundStereoEnergy,
  waitForInboundStereoQuiet,
} from './mix-energy-helpers.js'
import { defaultIceConfig, delay, waitForConnection } from './mix-three-client-helpers.js'

const ENERGY_PROBE_MS = 400
const LOUD_MIC_ENERGY_THRESHOLD = 500
const LOUD_MIC_ENERGY_WAIT_MS = 18_000
const TTS_ENERGY_THRESHOLD = 200
const TTS_ENERGY_WAIT_MS = 18_000
const TTS_QUIET_WINDOW_MS = 400
const TTS_QUIET_WAIT_MS = 20_000
const TTS_COUNTING_PHRASE = 'one two three four five six seven eight nine ten'

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
const LISTENER_SESSION_ID = 'session-c2'
const LISTENER_PEER_ID = 'client-mix-2'

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

function closeClient(client: ConnectedClient): void {
  client.teardownNegotiate()
  client.pc.close()
  client.signaling.disconnect()
}

function configurePositionalMix(
  host: VoiceAgentSessionHost,
  pod: SessionPod,
  c1X: number,
  c3X: number,
): void {
  host.createMixGroup({
    id: 'all',
    clientIds: [...SESSION_IDS],
  })
  host.setPositionalMixing(true)
  host.setClientPose(LISTENER_SESSION_ID, centerPose)
  host.setClientPose('session-c1', poseAtX(c1X))
  host.setClientPose('session-c3', poseAtX(c3X))
  expect(pod.resolveParticipantId(LISTENER_SESSION_ID)).toBe(LISTENER_PEER_ID)
}

async function waitForVoiceClientActive(
  pod: SessionPod,
  clientId: string,
  timeoutMs = 15_000,
): Promise<void> {
  const resolved = pod.resolveParticipantId(clientId)
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    for (const sessionId of SESSION_IDS) {
      const host = getVoiceHostForSession(pod, sessionId)
      if (host?.isVoiceClientActive(resolved)) {
        return
      }
    }
    await delay(50)
  }
  throw new Error(`timed out waiting for active voice client ${clientId}`)
}

describe.skipIf(!sessionPodMixIntegrationNativeAvailable())(
  'SessionPod mix-smoke integration (staging energy probes A–F)',
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

    it('exclusive loud mic pan, mute gates, and mock TTS speak pan match staging mix-smoke', async () => {
      for (const sessionId of SESSION_IDS) {
        await pod.ensureSession(sessionId)
      }

      const [client1, client2, client3] = await Promise.all([
        connectClientToSession(wsUrl, 'session-c1', 'client-mix-1'),
        connectClientToSession(wsUrl, 'session-c2', 'client-mix-2'),
        connectClientToSession(wsUrl, 'session-c3', 'client-mix-3'),
      ])

      try {
        const listenerHost = getVoiceHostForSession(pod, LISTENER_SESSION_ID) as HostTestAccess
        expect(listenerHost).toBeDefined()
        configurePositionalMix(listenerHost, pod, 3, -3)

        await client1.mic.writeSample(Buffer.alloc(960), 5)
        await client3.mic.writeSample(Buffer.alloc(960), 5)
        await client2.mic.writeSample(Buffer.alloc(960), 5)

        for (const clientId of CLIENT_IDS) {
          await waitForVoiceClientActive(pod, clientId)
        }

        const listener = client2
        const listenerAgent = listenerHost.sessions.get(LISTENER_PEER_ID)?.agent
        expect(listenerAgent).toBeDefined()

        // Probe A — session-c1 loud mic (+x → right louder)
        const probeADurationMs = LOUD_MIC_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeA = pumpLoudMicFrames(
          (frame, duration) => client1.mic.writeSample(frame, duration),
          probeADurationMs,
        )
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: LOUD_MIC_ENERGY_THRESHOLD,
          timeoutMs: LOUD_MIC_ENERGY_WAIT_MS,
          label: 'probe A loud mic',
        })
        const energyA = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeA
        assertRightLouder(energyA.left, energyA.right)

        // Probe B — session-c3 loud mic (-x → left louder)
        const probeBDurationMs = LOUD_MIC_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeB = pumpLoudMicFrames(
          (frame, duration) => client3.mic.writeSample(frame, duration),
          probeBDurationMs,
        )
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: LOUD_MIC_ENERGY_THRESHOLD,
          timeoutMs: LOUD_MIC_ENERGY_WAIT_MS,
          label: 'probe B loud mic',
        })
        const energyB = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeB
        assertLeftLouder(energyB.left, energyB.right)

        // Probe C — global mute silences session-c1 loud mic
        await pod.setGlobalMute('session-c1', true)
        const statusMuted = pod.getClientMixStatus('session-c1')
        expect(statusMuted.globallyMuted).toBe(true)

        const probeC = pumpLoudMicFrames(
          (frame, duration) => client1.mic.writeSample(frame, duration),
          ENERGY_PROBE_MS,
        )
        const rmsC = accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeC
        const energyC = await rmsC
        assertMuchQuieter(energyC, energyA)

        await pod.setGlobalMute('session-c1', false)

        // Probe D — listener mute on session-c3 for session-c2
        await pod.setListenerMute(LISTENER_SESSION_ID, 'session-c3', true)

        const probeD = pumpLoudMicFrames(
          (frame, duration) => client3.mic.writeSample(frame, duration),
          ENERGY_PROBE_MS,
        )
        const rmsD = accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeD
        const energyD = await rmsD
        assertMuchQuieter(energyD, energyB)

        await pod.setListenerMute(LISTENER_SESSION_ID, 'session-c3', false)

        // Drain leftover mic energy before TTS (staging probe E precondition)
        await waitForInboundStereoQuiet(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          quietWindowMs: TTS_QUIET_WINDOW_MS,
          timeoutMs: TTS_QUIET_WAIT_MS,
          label: 'mix inbound quiet before probe E',
        })

        // Probe E — mock TTS via speak(), panned right (+x)
        listenerHost.setTtsPose(LISTENER_SESSION_ID, poseAtX(3))
        const ttsPoseStatus = listenerHost.getClientMixStatus(LISTENER_PEER_ID)
        expect(ttsPoseStatus.ttsPose?.position.x).toBe(3)

        await listenerAgent!.sendTextToTTS(TTS_COUNTING_PHRASE)
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan probe E',
        })
        const energyE = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        assertRightLouder(energyE.left, energyE.right)

        await waitForInboundStereoQuiet(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          quietWindowMs: TTS_QUIET_WINDOW_MS,
          timeoutMs: TTS_QUIET_WAIT_MS,
          label: 'TTS drain after probe E',
        })

        // Probe F — mock TTS via speak(), panned left (-x)
        listenerHost.setTtsPose(LISTENER_SESSION_ID, poseAtX(-3))
        await listenerAgent!.sendTextToTTS(TTS_COUNTING_PHRASE)
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan probe F',
        })
        const energyF = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        assertLeftLouder(energyF.left, energyF.right)
      } finally {
        closeClient(client1)
        closeClient(client2)
        closeClient(client3)
        await delay(100)
      }
    }, 180_000)
  },
)
