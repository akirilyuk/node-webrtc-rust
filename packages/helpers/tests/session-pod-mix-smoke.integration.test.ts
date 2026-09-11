/**
 * SessionPod mix-smoke integration — mirrors staging e2e voice-data-mix-smoke
 * energy probes A–F (exclusive loud mic L/R pan, global/listener mute, TTS pan).
 * Also covers leftover left-loud mic drain: after probe-B-style talker at −x stops,
 * inbound must go quiet before TTS panned +x is right-loud on the summed mix pump.
 * Uses real RTCPeerConnections + native AudioMixGraph (not Goertzel simultaneous-sine).
 *
 * Verify:
 *   cd node-webrtc-rust
 *   npx vitest run packages/helpers/tests/session-pod-mix-smoke.integration.test.ts
 */
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { LocalAudioTrack, RemoteAudioTrack, RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { AudioMixGraph, quatIdentity, vec3Zero } from '@node-webrtc-rust/sdk/mix'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import type { ClientAudioMixer } from '../src/client-audio-mixer.js'
import { SessionPod } from '../src/session-pod.js'
import type { VoiceAgentSessionHost } from '../src/voice-agent-session-host.js'
import { VoiceSessionBudget, resetProcessVoiceSessionBudget } from '../src/voice-session-budget.js'
import {
  accumulateInboundStereoRms,
  accumulateDirectionalStereoRms,
  assertLeftLouder,
  assertMuchQuieter,
  assertRightLouder,
  pumpLoudMicFrames,
  pumpLoudTtsLeftOnlySidecarFrames,
  pumpLoudTtsSidecarFrames,
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

type SessionPodSlot = {
  sessionId: string
  signaling: SignalingClient
  host: VoiceAgentSessionHost
}

type SessionPodTestAccess = SessionPod & {
  slots: Map<string, SessionPodSlot>
}

type VoiceHostTestAccess = VoiceAgentSessionHost & {
  getClientMixer(): ClientAudioMixer | undefined
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

    it('exclusive loud mic pan, mute gates, and TTS sidecar pan match staging mix-smoke', async () => {
      for (const sessionId of SESSION_IDS) {
        await pod.ensureSession(sessionId)
      }

      const [client1, client2, client3] = await Promise.all([
        connectClientToSession(wsUrl, 'session-c1', 'client-mix-1'),
        connectClientToSession(wsUrl, 'session-c2', 'client-mix-2'),
        connectClientToSession(wsUrl, 'session-c3', 'client-mix-3'),
      ])

      try {
        const host = getVoiceHostForSession(pod, 'session-c2')
        expect(host).toBeDefined()
        configurePositionalMix(host!, 3, -3)

        await client1.mic.writeSample(Buffer.alloc(960), 5)
        await client3.mic.writeSample(Buffer.alloc(960), 5)
        await client2.mic.writeSample(Buffer.alloc(960), 5)

        for (const clientId of CLIENT_IDS) {
          await waitForVoiceClientActive(pod, clientId)
        }

        const listener = client2

        // Probe A — client-1 loud mic (+x → right louder)
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

        // Probe B — client-3 loud mic (-x → left louder)
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

        // Probe C — global mute silences client-1 loud mic
        await pod.setGlobalMute('client-mix-1', true)
        const statusMuted = pod.getClientMixStatus('client-mix-1')
        expect(statusMuted.globallyMuted).toBe(true)

        const probeC = pumpLoudMicFrames(
          (frame, duration) => client1.mic.writeSample(frame, duration),
          ENERGY_PROBE_MS,
        )
        const rmsC = accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeC
        const energyC = await rmsC
        assertMuchQuieter(energyC, energyA)

        await pod.setGlobalMute('client-mix-1', false)

        // Probe D — listener mute on client-3
        await pod.setListenerMute('client-mix-2', 'client-mix-3', true)

        const probeD = pumpLoudMicFrames(
          (frame, duration) => client3.mic.writeSample(frame, duration),
          ENERGY_PROBE_MS,
        )
        const rmsD = accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeD
        const energyD = await rmsD
        assertMuchQuieter(energyD, energyB)

        await pod.setListenerMute('client-mix-2', 'client-mix-3', false)

        // Probe E — TTS sidecar panned right (+x)
        const listenerHost = host as VoiceHostTestAccess
        const mixer = listenerHost.getClientMixer()
        expect(mixer).toBeDefined()
        // Sidecar is wired by VoiceAgent attach; loud frames enter via native tee → pendingTts.
        mixer!.createTtsSidecar('client-mix-2')

        listenerHost.setTtsPose('client-mix-2', poseAtX(3))
        const probeEDurationMs = TTS_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeE = pumpLoudTtsSidecarFrames(mixer!, 'client-mix-2', probeEDurationMs)
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan probe E',
        })
        const energyE = await accumulateDirectionalStereoRms(
          listener.agentAudio,
          ENERGY_PROBE_MS,
          'right',
        )
        await probeE
        assertRightLouder(energyE.left, energyE.right)

        await waitForInboundStereoQuiet(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          quietWindowMs: TTS_QUIET_WINDOW_MS,
          timeoutMs: TTS_QUIET_WAIT_MS,
          label: 'TTS drain after probe E',
        })

        // Probe F — TTS sidecar panned left (-x)
        listenerHost.clearTtsPose('client-mix-2')
        listenerHost.setTtsPose('client-mix-2', poseAtX(-3))
        const probeFDurationMs = TTS_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeF = pumpLoudTtsSidecarFrames(mixer!, 'client-mix-2', probeFDurationMs)
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan probe F',
        })
        const energyF = await accumulateDirectionalStereoRms(
          listener.agentAudio,
          ENERGY_PROBE_MS,
          'left',
        )
        await probeF
        assertLeftLouder(energyF.left, energyF.right)
      } finally {
        closeClient(client1)
        closeClient(client2)
        closeClient(client3)
        await delay(100)
      }
    }, 180_000)

    it('TTS left-only sidecar pans right after setTtsPose +x', async () => {
      await pod.ensureSession('session-c1')
      await pod.ensureSession('session-c2')

      const [talker, listener] = await Promise.all([
        connectClientToSession(wsUrl, 'session-c1', 'client-mix-1'),
        connectClientToSession(wsUrl, 'session-c2', 'client-mix-2'),
      ])

      const pumpAbort = new AbortController()

      try {
        const host = getVoiceHostForSession(pod, 'session-c2')
        expect(host).toBeDefined()

        host!.createMixGroup({
          id: 'tts-left-only',
          clientIds: ['client-mix-1', 'client-mix-2'],
        })
        host!.setPositionalMixing(true)
        host!.setClientPose('client-mix-2', centerPose)
        host!.setClientPose('client-mix-1', centerPose)

        await talker.mic.writeSample(Buffer.alloc(960), 5)
        await listener.mic.writeSample(Buffer.alloc(960), 5)

        await waitForVoiceClientActive(pod, 'client-mix-1')
        await waitForVoiceClientActive(pod, 'client-mix-2')

        // Prime mix path after mega-test teardown (same sequence as leftover mic drain).
        const probeMicDurationMs = LOUD_MIC_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeMic = pumpLoudMicFrames(
          (frame, duration) => talker.mic.writeSample(frame, duration),
          probeMicDurationMs,
        )
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: LOUD_MIC_ENERGY_THRESHOLD,
          timeoutMs: LOUD_MIC_ENERGY_WAIT_MS,
          label: 'left-only prime talker mic',
        })
        await probeMic

        await waitForInboundStereoQuiet(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          quietWindowMs: TTS_QUIET_WINDOW_MS,
          timeoutMs: TTS_QUIET_WAIT_MS,
          label: 'left-only prime mic drain',
        })

        const listenerHost = host as VoiceHostTestAccess
        const mixer = listenerHost.getClientMixer()
        expect(mixer).toBeDefined()
        mixer!.createTtsSidecar('client-mix-2')

        listenerHost.setTtsPose('client-mix-2', poseAtX(3))
        const probeDurationMs = TTS_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probe = pumpLoudTtsLeftOnlySidecarFrames(mixer!, 'client-mix-2', probeDurationMs, {
          signal: pumpAbort.signal,
        })
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'left-only TTS pan +x',
        })
        const energy = await accumulateDirectionalStereoRms(
          listener.agentAudio,
          ENERGY_PROBE_MS,
          'right',
        )
        await probe
        assertRightLouder(energy.left, energy.right)
      } finally {
        pumpAbort.abort()
        closeClient(talker)
        closeClient(listener)
        await delay(100)
      }
    }, 180_000)

    it('e2e parity: TTS probes E/F via session UUID setTtsPose and sendTextToTTS', async () => {
      for (const sessionId of SESSION_IDS) {
        await pod.ensureSession(sessionId)
      }

      const [client1, client2, client3] = await Promise.all([
        connectClientToSession(wsUrl, 'session-c1', 'client-mix-1'),
        connectClientToSession(wsUrl, 'session-c2', 'client-mix-2'),
        connectClientToSession(wsUrl, 'session-c3', 'client-mix-3'),
      ])

      try {
        const driverHost = getVoiceHostForSession(pod, 'session-c1')!
        const listenerHost = getVoiceHostForSession(pod, 'session-c2') as VoiceHostTestAccess
        expect(listenerHost).toBeDefined()
        configurePositionalMix(listenerHost, 3, -3)

        await client1.mic.writeSample(Buffer.alloc(960), 5)
        await client2.mic.writeSample(Buffer.alloc(960), 5)
        await client3.mic.writeSample(Buffer.alloc(960), 5)

        for (const clientId of CLIENT_IDS) {
          await waitForVoiceClientActive(pod, clientId)
        }

        const listener = client2

        for (const client of [client1, client2, client3]) {
          client.mic.writeSample(Buffer.alloc(960), 5).catch(() => undefined)
        }

        await waitForInboundStereoQuiet(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          quietWindowMs: TTS_QUIET_WINDOW_MS,
          timeoutMs: TTS_QUIET_WAIT_MS,
          label: 'mix inbound quiet before probe E',
        })

        driverHost.setTtsPose('session-c2', poseAtX(3))
        const agent = listenerHost.sessions.get('client-mix-2')?.agent
        expect(agent).toBeDefined()

        const probeE = agent!.sendTextToTTS('one two three four five')
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan probe E (session UUID pose)',
        })
        const energyE = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeE
        assertRightLouder(energyE.left, energyE.right)

        await waitForInboundStereoQuiet(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          quietWindowMs: TTS_QUIET_WINDOW_MS,
          timeoutMs: TTS_QUIET_WAIT_MS,
          label: 'TTS drain after probe E',
        })

        driverHost.clearTtsPose('session-c2')
        driverHost.setTtsPose('session-c2', poseAtX(-3))
        const probeF = agent!.sendTextToTTS('one two three four five')
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan probe F (session UUID pose)',
        })
        const energyF = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeF
        assertLeftLouder(energyF.left, energyF.right)
      } finally {
        closeClient(client1)
        closeClient(client2)
        closeClient(client3)
        await delay(100)
      }
    }, 180_000)

    it('leftover left-loud mic drains quiet before TTS panned right on mix pump', async () => {
      await pod.ensureSession('session-c1')
      await pod.ensureSession('session-c2')

      const [talker, listener] = await Promise.all([
        connectClientToSession(wsUrl, 'session-c1', 'client-mix-1'),
        connectClientToSession(wsUrl, 'session-c2', 'client-mix-2'),
      ])

      try {
        const host = getVoiceHostForSession(pod, 'session-c2')
        expect(host).toBeDefined()

        host!.createMixGroup({
          id: 'all',
          clientIds: ['client-mix-1', 'client-mix-2'],
        })
        host!.setPositionalMixing(true)
        host!.setClientPose('client-mix-2', centerPose)
        host!.setClientPose('client-mix-1', poseAtX(-3))

        await talker.mic.writeSample(Buffer.alloc(960), 5)
        await listener.mic.writeSample(Buffer.alloc(960), 5)

        await waitForVoiceClientActive(pod, 'client-mix-1')
        await waitForVoiceClientActive(pod, 'client-mix-2')

        const probeMicDurationMs = LOUD_MIC_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeMic = pumpLoudMicFrames(
          (frame, duration) => talker.mic.writeSample(frame, duration),
          probeMicDurationMs,
        )
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: LOUD_MIC_ENERGY_THRESHOLD,
          timeoutMs: LOUD_MIC_ENERGY_WAIT_MS,
          label: 'left-loud talker mic',
        })
        const energyMic = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
        await probeMic
        assertLeftLouder(energyMic.left, energyMic.right)

        await waitForInboundStereoQuiet(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          quietWindowMs: TTS_QUIET_WINDOW_MS,
          timeoutMs: TTS_QUIET_WAIT_MS,
          label: 'left-loud mic drain after talker stop',
        })

        const listenerHost = host as VoiceHostTestAccess
        const mixer = listenerHost.getClientMixer()
        expect(mixer).toBeDefined()
        mixer!.createTtsSidecar('client-mix-2')

        listenerHost.setTtsPose('client-mix-2', poseAtX(3))
        const probeTtsDurationMs = TTS_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeTts = pumpLoudTtsSidecarFrames(mixer!, 'client-mix-2', probeTtsDurationMs)
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan right after mic drain',
        })
        const energyTts = await accumulateDirectionalStereoRms(
          listener.agentAudio,
          ENERGY_PROBE_MS,
          'right',
        )
        await probeTts
        assertRightLouder(energyTts.left, energyTts.right)
      } finally {
        closeClient(talker)
        closeClient(listener)
        await delay(100)
      }
    }, 180_000)
  },
)
