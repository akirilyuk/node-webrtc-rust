/**
 * Sherpa 5-session concurrent echo + LID — ultimate-tier pool env, VoiceAgentSessionHost echo path.
 *
 * Five speaker↔echo pairs in one process (shared SherpaModelPool). Echo agents run inside
 * {@link VoiceAgentSessionHost} with async `onSpeechEvent` + 100 ms IPC-like delay before TTS.
 *
 *   npm run start:roundtrip-counting-echo-lid-multi --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SESSIONS                         concurrent pairs (default 5)
 *   SHERPA_MULTI_PCM_CAPTURE         per-hop PCM table (default on; set 0 to disable)
 *   SHERPA_ROUNDTRIP_WALL_MS         process wall clock (default 180000)
 *   SHERPA_COUNTING_*                same as counting echo LID sibling scripts
 */

import { LocalAudioTrack, RTCPeerConnection, type RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  SPEECH_EVENT_TYPE,
  type SpeechEvent,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'
import {
  VOICE_AGENT_SERVER_PEER_ID,
  VoiceAgentSessionHost,
  VoiceSessionBudget,
  type VoiceSessionHandler,
} from '@node-webrtc-rust/helpers'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { createKickFrame, PCM_KICK_DURATION_MS } from '../../shared/pcm-streaming.js'
import { DEMO_ICE_SERVERS, waitForConnection } from '../../shared/webrtc-demo-helpers.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig, withRoundtripHarnessSilence } from './resolve-voice-config.js'
import {
  DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
  DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
  AgentSpeakingEndLatch,
  ListenerUtteranceCollector,
  installRoundtripWallClockTimeout,
  playSpeakerTtsWithPostSilence,
  postTtsSilenceSeconds,
  sttFinalizeWaitMs,
  waitAgentPlaybackEndRace,
} from './roundtrip-counting.js'
import { echoVadConfig } from './roundtrip-counting-echo.js'
import {
  CLOUD_DEFAULT_LID_MIN_SPEECH_MS,
  formatEchoSmokeReply,
} from './roundtrip-counting-echo-lid-prefix.js'
import {
  assertFullCountingEcho,
  evaluateEchoLidEventOrdering,
} from './roundtrip-counting-echo-lid-multi-assert.js'
import {
  DEFAULT_VAD_ENERGY_THRESHOLD,
  MultiSessionPcmCapture,
  formatPcmFailureVerdicts,
  formatPcmHopTable,
  isMultiPcmCaptureEnabled,
  localizePcmFailure,
  wrapInboundTrackForPcmCapture,
  wrapOutboundTrackForPcmCapture,
  type PcmFailureVerdict,
  type PcmHopMetrics,
} from './roundtrip-counting-echo-lid-multi-pcm-capture.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { resolveLidModelPath } from './roundtrip-language-id.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import type { LifecycleSpeechEvent } from './roundtrip-stt-lifecycle-helpers.js'

const DEFAULT_TIMEOUT_MS = 90_000
const DEFAULT_WARMUP_S = 0.6
const DEFAULT_SESSIONS = 5
const ECHO_IPC_DELAY_MS = 100

function sleepMs(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function echoHostConfig(base: VoiceAgentConfig, lidModelPath: string): VoiceAgentConfig {
  return withRoundtripHarnessSilence({
    ...base,
    languageId: {
      modelPath: lidModelPath,
      minSpeechMs: CLOUD_DEFAULT_LID_MIN_SPEECH_MS,
    },
    events: { mode: 'stream' },
    vad: echoVadConfig(base),
  })
}

function speakerConfig(base: VoiceAgentConfig): VoiceAgentConfig {
  return withRoundtripHarnessSilence({
    ...base,
    events: { mode: 'stream' },
    vad: echoVadConfig(base),
  })
}

class EchoSpeechEventRecorder {
  private events: LifecycleSpeechEvent[] = []
  private t0 = Date.now()
  readonly endLatch = new AgentSpeakingEndLatch()

  constructor(
    private readonly label: string,
    private readonly onEvent?: (event: SpeechEvent) => void,
  ) {}

  reset(): void {
    this.events = []
    this.t0 = Date.now()
  }

  observe(event: SpeechEvent): void {
    logRoundtripSpeechEvent(this.label, event)
    this.endLatch.observe(event)
    this.onEvent?.(event)
    this.events.push({
      type: event.type,
      atMs: Date.now() - this.t0,
      text: event.text,
    })
  }

  snapshot(): LifecycleSpeechEvent[] {
    return [...this.events]
  }

  detectedLanguage(): string | null {
    for (let i = this.events.length - 1; i >= 0; i--) {
      const event = this.events[i]!
      if (event.type === SPEECH_EVENT_TYPE.userLanguage) {
        return event.text?.trim() || null
      }
    }
    return null
  }
}

interface SessionRuntime {
  sessionId: string
  peerId: string
  speaker: VoiceAgent
  speakerMic: LocalAudioTrack
  collector: ListenerUtteranceCollector
  speakerEndLatch: AgentSpeakingEndLatch
  echoRecorder: EchoSpeechEventRecorder
  pcmCapture: MultiSessionPcmCapture | null
  host: VoiceAgentSessionHost
  cleanup: () => Promise<void>
}

async function waitForVoiceClientActive(
  host: VoiceAgentSessionHost,
  clientId: string,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (host.isVoiceClientActive(clientId)) {
      return
    }
    await sleepMs(50)
  }
  throw new Error(`timed out waiting for active voice client ${clientId}`)
}

async function setupSession(params: {
  sessionIndex: number
  wsUrl: string
  echoConfig: VoiceAgentConfig
  speakerBase: VoiceAgentConfig
  sessionBudget: VoiceSessionBudget
  verbose: boolean
  connectTimeoutMs: number
  pcmCaptureEnabled: boolean
  vadThreshold: number
}): Promise<SessionRuntime> {
  const sessionId = `s${params.sessionIndex + 1}`
  const peerId = `client-${sessionId}`
  const room = `lid-echo-multi-${sessionId}`
  const pcmCapture = params.pcmCaptureEnabled
    ? new MultiSessionPcmCapture(params.vadThreshold)
    : null
  let replyCapturePending = false

  let peerConnectedResolve!: () => void
  const peerConnected = new Promise<void>((resolve) => {
    peerConnectedResolve = resolve
  })

  const echoRecorder = new EchoSpeechEventRecorder(`echo-${sessionId}`, (event) => {
    if (!pcmCapture) return
    if (event.type === SPEECH_EVENT_TYPE.userSpeechFinal) {
      replyCapturePending = true
    }
    if (replyCapturePending && event.type === SPEECH_EVENT_TYPE.agentSpeakingStart) {
      replyCapturePending = false
      pcmCapture.startEchoOutboundCapture()
      pcmCapture.startRxCapture()
    }
    if (event.type === SPEECH_EVENT_TYPE.agentSpeakingEnd) {
      pcmCapture.stopEchoOutboundCapture()
    }
  })

  const voiceHandler: VoiceSessionHandler = {
    onPeerConnected: () => {
      peerConnectedResolve()
    },
    async onSpeechEvent(_ctx, event) {
      echoRecorder.observe(event)
      if (event.type !== SPEECH_EVENT_TYPE.userSpeechFinal) {
        return
      }
      const text = event.text?.trim()
      if (!text) {
        return
      }
      await sleepMs(ECHO_IPC_DELAY_MS)
      await _ctx.speak(formatEchoSmokeReply(text))
    },
  }

  const serverSignaling = new SignalingClient({
    url: params.wsUrl,
    room,
    peerId: VOICE_AGENT_SERVER_PEER_ID,
  })
  await serverSignaling.connect()

  const host = new VoiceAgentSessionHost(serverSignaling, DEMO_ICE_SERVERS, {
    voiceConfig: params.echoConfig,
    voiceHandler,
    sessionBudget: params.sessionBudget,
    wrapAudioTracks: pcmCapture
      ? ({ outbound }) => ({
          outbound: wrapOutboundTrackForPcmCapture(outbound, pcmCapture),
        })
      : undefined,
  })

  const clientPc = new RTCPeerConnection({ iceServers: DEMO_ICE_SERVERS })
  const speakerMic = new LocalAudioTrack(`mic-${sessionId}`, `stream-${sessionId}`)
  await clientPc.addTrack(speakerMic)

  const clientSignaling = new SignalingClient({
    url: params.wsUrl,
    room,
    peerId,
  })
  autoNegotiate({ pc: clientPc, signaling: clientSignaling, polite: true })

  const agentAudioPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out waiting for agent audio (${sessionId})`)),
      params.connectTimeoutMs,
    )
    clientPc.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        clearTimeout(timer)
        resolve(event.track as RemoteAudioTrack)
      }
    }
  })

  await clientSignaling.connect()
  await waitForConnection(clientPc, params.connectTimeoutMs)
  await speakerMic.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)

  let agentAudio = await agentAudioPromise
  if (pcmCapture) {
    agentAudio = wrapInboundTrackForPcmCapture(agentAudio, pcmCapture)
  }

  await Promise.race([
    peerConnected,
    sleepMs(params.connectTimeoutMs).then(() => {
      throw new Error(`timed out waiting for echo host peer connected (${sessionId})`)
    }),
  ])
  await waitForVoiceClientActive(host, peerId, params.connectTimeoutMs)

  const speaker = new VoiceAgent(speakerConfig(params.speakerBase))
  await speaker.attach({ inboundTrack: agentAudio, outboundTrack: speakerMic })
  await speaker.start()

  const pumpStarted = { value: false }
  const speakerEndLatch = new AgentSpeakingEndLatch()
  const collector = new ListenerUtteranceCollector(
    speaker,
    pumpStarted,
    params.verbose,
    `speaker-${sessionId}`,
    speakerEndLatch,
  )
  collector.startPump()

  await streamSilence(speakerMic, DEFAULT_WARMUP_S)

  return {
    sessionId,
    peerId,
    speaker,
    speakerMic,
    collector,
    speakerEndLatch,
    echoRecorder,
    pcmCapture,
    host,
    cleanup: async () => {
      await speaker.stop().catch(() => undefined)
      await host.close().catch(() => undefined)
      clientPc.close()
      clientSignaling.disconnect()
      serverSignaling.disconnect()
    },
  }
}

export interface SessionRoundResult {
  sessionId: string
  recognized: string
  language: string | null
  orderingOk: boolean
  failures: string[]
  pcmMetrics: PcmHopMetrics | null
  pcmVerdict: PcmFailureVerdict | null
}

async function runSessionRound(params: {
  session: SessionRuntime
  countingPhrase: string
  timeoutMs: number
  finalizeWaitMs: number
  postTtsSilenceS: number
}): Promise<SessionRoundResult> {
  const { session, countingPhrase, timeoutMs, finalizeWaitMs, postTtsSilenceS } = params
  const failures: string[] = []

  session.echoRecorder.reset()
  session.pcmCapture?.resetRound()
  session.collector.startEventRecording()

  const echoEndBaseline = session.echoRecorder.endLatch.endEventsSeen()

  const playbackPromise = playSpeakerTtsWithPostSilence({
    speaker: session.speaker,
    speakerOut: session.speakerMic,
    phrase: countingPhrase,
    postTtsSilenceS,
    playbackTimeoutMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    agentSpeakingEndLatch: session.speakerEndLatch,
  })

  const playbackAndEchoDone = playbackPromise.then(async () => {
    await waitAgentPlaybackEndRace({
      phrase: formatEchoSmokeReply(countingPhrase),
      capMs: timeoutMs,
      waitForAgentSpeakingEnd: () =>
        session.echoRecorder.endLatch.waitAfterCount(echoEndBaseline, timeoutMs),
    })
    if (postTtsSilenceS > 0) {
      await streamSilence(session.speakerMic, postTtsSilenceS)
    }
  })

  const recognized = await session.collector.waitForNextAfterPlayback(
    playbackAndEchoDone,
    timeoutMs,
    finalizeWaitMs,
  )
  await playbackAndEchoDone

  session.pcmCapture?.stopRxCapture()
  session.pcmCapture?.stopEchoOutboundCapture()

  const speakerEvents = session.collector.stopEventRecording()
  for (const event of speakerEvents) {
    session.pcmCapture?.observeSpeakerSpeechEvent(event.type, event.atMs)
  }

  const best = session.collector.stats.finals.reduce(
    (a, b) => (a.trim().length >= b.trim().length ? a : b),
    recognized,
  )
  const inboundTranscript = best.trim().length > recognized.trim().length ? best.trim() : recognized

  const echoAssert = assertFullCountingEcho(inboundTranscript)
  failures.push(...echoAssert.failures.map((f) => `${session.sessionId} inbound: ${f}`))

  const ordering = evaluateEchoLidEventOrdering({
    events: session.echoRecorder.snapshot(),
    label: session.sessionId,
  })
  if (!ordering.passed) {
    failures.push(...ordering.failures)
  }

  const pcmMetrics = session.pcmCapture?.metrics(inboundTranscript) ?? null
  const pcmVerdict =
    pcmMetrics != null && failures.length > 0 ? localizePcmFailure(pcmMetrics) : null

  return {
    sessionId: session.sessionId,
    recognized: inboundTranscript,
    language: session.echoRecorder.detectedLanguage(),
    orderingOk: ordering.passed,
    failures,
    pcmMetrics,
    pcmVerdict,
  }
}

function printSummaryTable(results: SessionRoundResult[]): void {
  console.log('')
  console.log('=== Per-session summary ===')
  console.log('session | ordering | language | recognized (prefix)')
  for (const row of results) {
    const preview = row.recognized.length > 72 ? `${row.recognized.slice(0, 72)}…` : row.recognized
    console.log(
      `${row.sessionId.padEnd(7)} | ${row.orderingOk ? 'OK' : 'FAIL'.padEnd(7)} | ${(row.language ?? '—').padEnd(8)} | ${preview}`,
    )
  }
}

function printPcmTables(results: SessionRoundResult[]): void {
  const pcmRows = results
    .filter((row) => row.pcmMetrics != null)
    .map((row) => ({ sessionId: row.sessionId, metrics: row.pcmMetrics! }))
  if (pcmRows.length === 0) return
  console.log('')
  console.log(formatPcmHopTable(pcmRows))

  const failingPcm = results
    .filter((row) => row.failures.length > 0 && row.pcmMetrics != null && row.pcmVerdict != null)
    .map((row) => ({
      sessionId: row.sessionId,
      metrics: row.pcmMetrics!,
      verdict: row.pcmVerdict!,
    }))
  if (failingPcm.length > 0) {
    console.log('')
    console.log(formatPcmFailureVerdicts(failingPcm))
  }
}

async function main(): Promise<void> {
  const sessionCount = Math.max(1, Math.min(10, Number(process.env.SESSIONS ?? DEFAULT_SESSIONS)))
  if (!process.env.VOICE_MAX_CONCURRENT_SESSIONS) {
    process.env.VOICE_MAX_CONCURRENT_SESSIONS = String(sessionCount + 4)
  }

  installRoundtripWallClockTimeout(Number(process.env.SHERPA_ROUNDTRIP_WALL_MS) || 180_000)

  const countingPhrase =
    process.env.SHERPA_COUNTING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const { config: base, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  const lidModelPath = resolveLidModelPath()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const finalizeWaitMs = sttFinalizeWaitMs(base)
  const postTtsSilenceS = postTtsSilenceSeconds(base)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'
  const connectTimeoutMs = Math.min(timeoutMs, 45_000)
  const echoConfig = echoHostConfig(base, lidModelPath)
  const sessionBudget = new VoiceSessionBudget(0)
  const pcmCaptureEnabled = isMultiPcmCaptureEnabled()
  const vadThreshold = base.vad?.threshold ?? DEFAULT_VAD_ENERGY_THRESHOLD

  console.log('=== Sherpa 5-session concurrent echo + LID (VoiceAgentSessionHost) ===')
  console.log(`Pipeline: ${label}`)
  console.log(`Sessions: ${sessionCount} concurrent (Promise.all connect + rounds)`)
  console.log(`SHERPA_MULTI_PCM_CAPTURE=${pcmCaptureEnabled ? '1' : '0'}`)
  console.log(
    `Pool env: SHERPA_POOL_MAX_CONCURRENT_DECODE=${process.env.SHERPA_POOL_MAX_CONCURRENT_DECODE ?? '(default)'} SHERPA_STT_NUM_THREADS=${process.env.SHERPA_STT_NUM_THREADS ?? '(default)'} SHERPA_POOL_MAX_CONCURRENT_TTS=${process.env.SHERPA_POOL_MAX_CONCURRENT_TTS ?? '(default)'} SHERPA_TTS_NUM_THREADS=${process.env.SHERPA_TTS_NUM_THREADS ?? '(default)'}`,
  )
  console.log(
    `VAD: gateStt=${base.vad?.gateStt !== false}  minSilence=${base.vad?.minSilenceDurationMs ?? VOICE_AGENT_VAD_PRESET.minSilenceDurationMs}ms  threshold=${vadThreshold}`,
  )
  console.log(`STT: ${sttModelPath}`)
  console.log(`TTS: ${ttsModelPath}`)
  console.log(`LID: ${lidModelPath}  minSpeechMs=${CLOUD_DEFAULT_LID_MIN_SPEECH_MS}`)
  console.log(`Speaker phrase: "${countingPhrase}"`)
  console.log(`Echo IPC delay: ${ECHO_IPC_DELAY_MS}ms before ctx.speak`)
  console.log('')

  const signalingServer = new SignalingServer({ port: 0 })
  await signalingServer.listen(0)
  const wsUrl = `ws://127.0.0.1:${signalingServer.port}`

  const sessions = await Promise.all(
    Array.from({ length: sessionCount }, (_, index) =>
      setupSession({
        sessionIndex: index,
        wsUrl,
        echoConfig,
        speakerBase: base,
        sessionBudget,
        verbose,
        connectTimeoutMs,
        pcmCaptureEnabled,
        vadThreshold,
      }),
    ),
  )

  let results: SessionRoundResult[]
  try {
    results = await Promise.all(
      sessions.map((session) =>
        runSessionRound({
          session,
          countingPhrase,
          timeoutMs,
          finalizeWaitMs,
          postTtsSilenceS,
        }),
      ),
    )
  } finally {
    await Promise.all(sessions.map((session) => session.cleanup().catch(() => undefined)))
    await signalingServer.close().catch(() => undefined)
  }

  printSummaryTable(results)
  printPcmTables(results)

  const allFailures = results.flatMap((result) => result.failures)
  if (allFailures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: 'concurrent echo + LID multi-session assertions failed',
      failures: allFailures,
      legs: results.map((result) => ({
        label: result.sessionId,
        phrase: countingPhrase,
        recognized: result.recognized,
      })),
    })
  }

  console.log(
    `\nConcurrent echo + LID roundtrip OK — ${sessionCount}/${sessionCount} sessions full echo one..ten with LID ordering.`,
  )
  process.exit(0)
}

const isMain = process.argv[1]?.endsWith('roundtrip-counting-echo-lid-multi.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    exitSherpaRoundtripFailure({
      reason: 'uncaught error',
      error,
    })
  })
}
