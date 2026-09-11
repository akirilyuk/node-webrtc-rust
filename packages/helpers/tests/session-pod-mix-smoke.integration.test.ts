/**
 * SessionPod mix-smoke integration — mirrors staging e2e voice-data-mix-smoke probes A–F.
 * Uses orchestrator session UUIDs, accumulateInboundStereoRms only, dual-mono TTS sidecar for E/F.
 *
 * Verify:
 *   cd node-webrtc-rust
 *   npm run test:integration --workspace=@node-webrtc-rust/helpers
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
  assertLeftLouder,
  assertMuchQuieter,
  assertRightLouder,
  pumpLoudMicFrames,
  pumpLoudTtsSidecarFrames,
  waitForInboundStereoEnergy,
  waitForInboundStereoQuiet,
} from './mix-energy-helpers.js'
import { defaultIceConfig, delay, waitForConnection } from './mix-three-client-helpers.js'
import { SMOKE_PEER_IDS, SMOKE_SESSION_UUIDS } from './smoke-parity-helpers.js'

const ENERGY_PROBE_MS = 400
const LOUD_MIC_ENERGY_THRESHOLD = 500
const LOUD_MIC_ENERGY_WAIT_MS = 18_000
const TTS_ENERGY_THRESHOLD = 200
const TTS_ENERGY_WAIT_MS = 18_000
const TTS_QUIET_WINDOW_MS = 400
const TTS_QUIET_WAIT_MS = 20_000
/** Finite speak stand-in (staging Piper utterance) — not an 18s continuous pump. */
const TTS_SPEAK_STANDIN_MS = 6_000

type SessionPodSlot = {
  sessionId: string
  signaling: SignalingClient
  host: VoiceAgentSessionHost
}

type SessionPodTestAccess = SessionPod & {
  slots: Map<string, SessionPodSlot>
}

type VoiceHostSessionState = {
  agentTtsOut?: { setWriteSampleTee(callback: ((...args: unknown[]) => void) | null): void }
}

type VoiceHostTestAccess = VoiceAgentSessionHost & {
  getClientMixer(): ClientAudioMixer | undefined
  sessions: Map<string, VoiceHostSessionState>
}

function detachAgentTtsTee(host: VoiceHostTestAccess, peerId: string): void {
  const sidecar = host.sessions.get(peerId)?.agentTtsOut
  sidecar?.setWriteSampleTee(null)
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

async function waitForVoiceClientActive(
  pod: SessionPod,
  clientId: string,
  timeoutMs = 15_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    for (const sessionId of SMOKE_SESSION_UUIDS) {
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

    it(
      'voice-data-mix-smoke parity: probes A–F with session UUIDs and dual-mono TTS',
      { timeout: 180_000, retry: 2 },
      async () => {
        for (const sessionId of SMOKE_SESSION_UUIDS) {
          await pod.ensureSession(sessionId)
        }

        const [client1, client2, client3] = await Promise.all([
          connectClientToSession(wsUrl, SMOKE_SESSION_UUIDS[0], SMOKE_PEER_IDS[0]),
          connectClientToSession(wsUrl, SMOKE_SESSION_UUIDS[1], SMOKE_PEER_IDS[1]),
          connectClientToSession(wsUrl, SMOKE_SESSION_UUIDS[2], SMOKE_PEER_IDS[2]),
        ])

        try {
          const driverHost = getVoiceHostForSession(pod, SMOKE_SESSION_UUIDS[0])!
          const listenerHost = getVoiceHostForSession(
            pod,
            SMOKE_SESSION_UUIDS[1],
          ) as VoiceHostTestAccess
          expect(listenerHost).toBeDefined()

          driverHost.createMixGroup({ id: 'all', clientIds: [...SMOKE_SESSION_UUIDS] })
          driverHost.setPositionalMixing(true)
          driverHost.setClientPose(SMOKE_SESSION_UUIDS[1], centerPose)
          driverHost.setClientPose(SMOKE_SESSION_UUIDS[0], poseAtX(3))
          driverHost.setClientPose(SMOKE_SESSION_UUIDS[2], poseAtX(-3))

          await client1.mic.writeSample(Buffer.alloc(960), 5)
          await client2.mic.writeSample(Buffer.alloc(960), 5)
          await client3.mic.writeSample(Buffer.alloc(960), 5)

          for (const peerId of SMOKE_PEER_IDS) {
            await waitForVoiceClientActive(pod, peerId)
          }

          const listener = client2
          const listenerPeerId = SMOKE_PEER_IDS[1]
          const listenerUuid = SMOKE_SESSION_UUIDS[1]

          // Probe A — client-1 loud mic (+x → right louder on listener inbound)
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

          // Probe C — global mute on client-0 (session UUID)
          await pod.setGlobalMute(SMOKE_SESSION_UUIDS[0], true)
          const statusMuted = pod.getClientMixStatus(SMOKE_SESSION_UUIDS[0])
          expect(statusMuted.globallyMuted).toBe(true)

          const probeC = pumpLoudMicFrames(
            (frame, duration) => client1.mic.writeSample(frame, duration),
            ENERGY_PROBE_MS,
          )
          const rmsC = accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
          await probeC
          const energyC = await rmsC
          assertMuchQuieter(energyC, energyA)

          await pod.setGlobalMute(SMOKE_SESSION_UUIDS[0], false)

          // Probe D — listener mute on client-3 (session UUIDs)
          await pod.setListenerMute(SMOKE_SESSION_UUIDS[1], SMOKE_SESSION_UUIDS[2], true)

          const probeD = pumpLoudMicFrames(
            (frame, duration) => client3.mic.writeSample(frame, duration),
            ENERGY_PROBE_MS,
          )
          const rmsD = accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
          await probeD
          const energyD = await rmsD
          assertMuchQuieter(energyD, energyB)

          await pod.setListenerMute(SMOKE_SESSION_UUIDS[1], SMOKE_SESSION_UUIDS[2], false)

          // Silence other talkers during TTS pan probes (e2e pauses mic pumps; graph still routes poses).
          await pod.setGlobalMute(SMOKE_SESSION_UUIDS[0], true)
          await pod.setGlobalMute(SMOKE_SESSION_UUIDS[2], true)
          await client2.mic.writeSample(Buffer.alloc(960), 5)

          await waitForInboundStereoQuiet(listener.agentAudio, {
            threshold: TTS_ENERGY_THRESHOLD,
            quietWindowMs: TTS_QUIET_WINDOW_MS,
            timeoutMs: TTS_QUIET_WAIT_MS,
            label: 'mix inbound quiet before probe E',
          })

          const mixer = listenerHost.getClientMixer()
          expect(mixer).toBeDefined()
          mixer!.createTtsSidecar(listenerPeerId)
          detachAgentTtsTee(listenerHost, listenerPeerId)

          // Probe E — dual-mono TTS panned right (+x); mirrors e2e set_tts_pose → speak → energy → RMS
          await listenerHost.setTtsPose(listenerUuid, poseAtX(3))
          await waitForInboundStereoQuiet(listener.agentAudio, {
            threshold: TTS_ENERGY_THRESHOLD,
            quietWindowMs: TTS_QUIET_WINDOW_MS,
            timeoutMs: TTS_QUIET_WAIT_MS,
            label: 'mix inbound quiet after TTS pose +x (probe E)',
          })
          const probeE = pumpLoudTtsSidecarFrames(mixer!, listenerPeerId, TTS_SPEAK_STANDIN_MS)
          await waitForInboundStereoEnergy(listener.agentAudio, {
            threshold: TTS_ENERGY_THRESHOLD,
            timeoutMs: TTS_ENERGY_WAIT_MS,
            label: 'TTS pan probe E',
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

          // Probe F — dual-mono TTS panned left (-x); same e2e shape as E (no directional wait)
          await listenerHost.setTtsPose(listenerUuid, poseAtX(-3))
          await waitForInboundStereoQuiet(listener.agentAudio, {
            threshold: TTS_ENERGY_THRESHOLD,
            quietWindowMs: TTS_QUIET_WINDOW_MS,
            timeoutMs: TTS_QUIET_WAIT_MS,
            label: 'mix inbound quiet after TTS pose -x (probe F)',
          })
          const probeF = pumpLoudTtsSidecarFrames(mixer!, listenerPeerId, TTS_SPEAK_STANDIN_MS)
          await waitForInboundStereoEnergy(listener.agentAudio, {
            threshold: TTS_ENERGY_THRESHOLD,
            timeoutMs: TTS_ENERGY_WAIT_MS,
            label: 'TTS pan probe F',
          })
          const energyF = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
          await probeF
          assertLeftLouder(energyF.left, energyF.right)

          await listenerHost.clearTtsPose(listenerUuid)

          await pod.setGlobalMute(SMOKE_SESSION_UUIDS[0], false)
          await pod.setGlobalMute(SMOKE_SESSION_UUIDS[2], false)
        } finally {
          closeClient(client1)
          closeClient(client2)
          closeClient(client3)
          await delay(100)
        }
      },
    )

    it('leftover left-loud mic drains quiet before TTS panned right on mix pump', async () => {
      await pod.ensureSession(SMOKE_SESSION_UUIDS[0])
      await pod.ensureSession(SMOKE_SESSION_UUIDS[1])

      const [talker, listener] = await Promise.all([
        connectClientToSession(wsUrl, SMOKE_SESSION_UUIDS[0], SMOKE_PEER_IDS[0]),
        connectClientToSession(wsUrl, SMOKE_SESSION_UUIDS[1], SMOKE_PEER_IDS[1]),
      ])

      try {
        const host = getVoiceHostForSession(pod, SMOKE_SESSION_UUIDS[1])
        expect(host).toBeDefined()

        host!.createMixGroup({
          id: 'all',
          clientIds: [SMOKE_PEER_IDS[0], SMOKE_PEER_IDS[1]],
        })
        host!.setPositionalMixing(true)
        host!.setClientPose(SMOKE_PEER_IDS[1], centerPose)
        host!.setClientPose(SMOKE_PEER_IDS[0], poseAtX(-3))

        await talker.mic.writeSample(Buffer.alloc(960), 5)
        await listener.mic.writeSample(Buffer.alloc(960), 5)

        await waitForVoiceClientActive(pod, SMOKE_PEER_IDS[0])
        await waitForVoiceClientActive(pod, SMOKE_PEER_IDS[1])

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
        mixer!.createTtsSidecar(SMOKE_PEER_IDS[1])

        await listenerHost.setTtsPose(SMOKE_PEER_IDS[1], poseAtX(3))
        const probeTtsDurationMs = TTS_ENERGY_WAIT_MS + ENERGY_PROBE_MS
        const probeTts = pumpLoudTtsSidecarFrames(mixer!, SMOKE_PEER_IDS[1], probeTtsDurationMs)
        await waitForInboundStereoEnergy(listener.agentAudio, {
          threshold: TTS_ENERGY_THRESHOLD,
          timeoutMs: TTS_ENERGY_WAIT_MS,
          label: 'TTS pan right after mic drain',
        })
        const energyTts = await accumulateInboundStereoRms(listener.agentAudio, ENERGY_PROBE_MS)
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
