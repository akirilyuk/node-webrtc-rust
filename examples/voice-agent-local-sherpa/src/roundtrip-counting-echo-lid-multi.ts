/**
 * Sherpa 10-session concurrent echo + LID — split echo server + speaker clients (default).
 *
 * Production-shaped topology: echo agents run in a child process (runner-like pool env);
 * speaker clients run in the parent with a generous Sherpa pool.
 *
 *   npm run start:roundtrip-counting-echo-lid-multi --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Env:
 *   SESSIONS                         concurrent pairs (default 10 = ultimate maxActiveConnectionsPerPod)
 *   SHERPA_MULTI_SINGLE_PROCESS=1    single-process comparison mode (shared pool)
 *   SHERPA_MULTI_PCM_CAPTURE         per-hop PCM table (default on; set 0 to disable)
 *   SHERPA_MULTI_WAV_DIR             WAV dumps on failure (default .test-logs/multi-wav/<stamp>/)
 *   SHERPA_MULTI_WAV_ALL=1           also dump WAVs for passing sessions
 *   SHERPA_ROUNDTRIP_WALL_MS         process wall clock (default 240000)
 *   SHERPA_COUNTING_*                same as counting echo LID sibling scripts
 */

import type { ChildProcessWithoutNullStreams } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { LocalAudioTrack, RTCPeerConnection, type RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import {
  VoiceAgent,
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
  assertEchoOutboundPcmWithinSpeakingWindow,
  assertFullCountingEcho,
  evaluateEchoLidEventOrdering,
} from './roundtrip-counting-echo-lid-multi-assert.js'
import {
  DEFAULT_VAD_ENERGY_THRESHOLD,
  MultiSessionPcmCapture,
  formatBurstMetricTable,
  formatMergedHopTable,
  formatPcmFailureVerdicts,
  isMultiPcmCaptureEnabled,
  localizeBurstFailure,
  resolveMultiWavDir,
  transcriptMissingEchoPrefix,
  wrapInboundTrackForPcmCapture,
  wrapOutboundTrackForPcmCapture,
  type PcmFailureVerdict,
  type PcmHopMetrics,
} from './roundtrip-counting-echo-lid-multi-pcm-capture.js'
import {
  mergeEchoAndSpeakerMetrics,
  requestEchoEndBaseline,
  requestEchoMetrics,
  requestEchoWavWrites,
  sendParentCommand,
  shutdownEchoChild,
  waitEchoAgentSpeakingEnd,
  waitForEchoHostsListening,
  waitForEchoPeersReady,
  wireEchoServerChild,
  type EchoChildWire,
  type MergedHopMetrics,
} from './roundtrip-counting-echo-lid-multi-ipc.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { resolveLidModelPath } from './roundtrip-language-id.js'
import {
  enableRoundtripSpeechEventLog,
  logRoundtripSpeechEvent,
} from './roundtrip-speech-events.js'
import type { LifecycleSpeechEvent } from './roundtrip-stt-lifecycle-helpers.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const EXPORT_SHERPA_SCRIPT = join(__dirname, '../../../scripts/export-sherpa-local-models.sh')

const DEFAULT_TIMEOUT_MS = 90_000
const DEFAULT_WARMUP_S = 0.6
const DEFAULT_SESSIONS = 10
const ECHO_IPC_DELAY_MS = 100

const ECHO_POOL_ENV = {
  SHERPA_POOL_MAX_CONCURRENT_DECODE: '2',
  SHERPA_STT_NUM_THREADS: '1',
  SHERPA_POOL_MAX_CONCURRENT_TTS: '2',
  SHERPA_TTS_NUM_THREADS: '1',
}

let activeEchoChild: ChildProcessWithoutNullStreams | null = null

function registerEchoChildKillOnExit(child: ChildProcessWithoutNullStreams): void {
  activeEchoChild = child
  const kill = (): void => {
    if (activeEchoChild && !activeEchoChild.killed) {
      activeEchoChild.kill('SIGTERM')
    }
  }
  process.once('exit', kill)
  process.once('SIGINT', kill)
  process.once('SIGTERM', kill)
}

function languageFromEchoEvents(events: LifecycleSpeechEvent[]): string | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!
    if (event.type === SPEECH_EVENT_TYPE.userLanguage) {
      return event.text?.trim() || null
    }
  }
  return null
}

const SPEAKER_POOL_ENV = {
  SHERPA_POOL_MAX_CONCURRENT_DECODE: '10',
  SHERPA_STT_NUM_THREADS: '1',
  SHERPA_POOL_MAX_CONCURRENT_TTS: '10',
  SHERPA_TTS_NUM_THREADS: '1',
}

function sleepMs(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function isSingleProcessMode(env: NodeJS.ProcessEnv = process.env): boolean {
  const raw = env.SHERPA_MULTI_SINGLE_PROCESS
  return raw === '1' || raw?.toLowerCase() === 'true'
}

function applyPoolEnv(pool: Record<string, string>): void {
  for (const [key, value] of Object.entries(pool)) {
    process.env[key] = value
  }
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
    private readonly onEvent?: (event: SpeechEvent, atMs: number) => void,
  ) {}

  reset(): void {
    this.events = []
    this.t0 = Date.now()
  }

  observe(event: SpeechEvent): void {
    logRoundtripSpeechEvent(this.label, event)
    this.endLatch.observe(event)
    const atMs = Date.now() - this.t0
    this.events.push({
      type: event.type,
      atMs,
      text: event.text,
    })
    this.onEvent?.(event, atMs)
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

  agentSpeakingStartAt(): number | null {
    for (const event of this.events) {
      if (event.type === SPEECH_EVENT_TYPE.agentSpeakingStart) {
        return event.atMs
      }
    }
    return null
  }

  agentSpeakingEndAt(): number | null {
    let last: number | null = null
    for (const event of this.events) {
      if (event.type === SPEECH_EVENT_TYPE.agentSpeakingEnd) {
        last = event.atMs
      }
    }
    return last
  }
}

interface SpeakerSessionRuntime {
  sessionId: string
  peerId: string
  speaker: VoiceAgent
  speakerMic: LocalAudioTrack
  collector: ListenerUtteranceCollector
  speakerEndLatch: AgentSpeakingEndLatch
  pcmCapture: MultiSessionPcmCapture | null
  cleanup: () => Promise<void>
}

interface FullSessionRuntime extends SpeakerSessionRuntime {
  echoRecorder: EchoSpeechEventRecorder
  host: VoiceAgentSessionHost
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

async function setupSpeakerSession(params: {
  sessionIndex: number
  wsUrl: string
  speakerBase: VoiceAgentConfig
  verbose: boolean
  connectTimeoutMs: number
  pcmCaptureEnabled: boolean
  vadThreshold: number
}): Promise<SpeakerSessionRuntime> {
  const sessionId = `s${params.sessionIndex + 1}`
  const peerId = `client-${sessionId}`
  const room = `lid-echo-multi-${sessionId}`
  const pcmCapture = params.pcmCaptureEnabled
    ? new MultiSessionPcmCapture(params.vadThreshold)
    : null

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
    pcmCapture,
    cleanup: async () => {
      await speaker.stop().catch(() => undefined)
      clientPc.close()
      clientSignaling.disconnect()
    },
  }
}

async function setupFullSession(params: {
  sessionIndex: number
  wsUrl: string
  echoConfig: VoiceAgentConfig
  speakerBase: VoiceAgentConfig
  sessionBudget: VoiceSessionBudget
  verbose: boolean
  connectTimeoutMs: number
  pcmCaptureEnabled: boolean
  vadThreshold: number
}): Promise<FullSessionRuntime> {
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

  const echoRecorder = new EchoSpeechEventRecorder(`echo-${sessionId}`, (event, atMs) => {
    if (!pcmCapture) return
    if (event.type === SPEECH_EVENT_TYPE.userSpeechFinal) {
      replyCapturePending = true
    }
    if (replyCapturePending && event.type === SPEECH_EVENT_TYPE.agentSpeakingStart) {
      replyCapturePending = false
      pcmCapture.startEchoOutboundCapture()
      pcmCapture.startRxCapture()
    }
    if (event.type === SPEECH_EVENT_TYPE.agentSpeakingStart) {
      pcmCapture.observeAgentSpeakingStart(atMs)
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

  const speakerPart = await setupSpeakerSession({
    sessionIndex: params.sessionIndex,
    wsUrl: params.wsUrl,
    speakerBase: params.speakerBase,
    verbose: params.verbose,
    connectTimeoutMs: params.connectTimeoutMs,
    pcmCaptureEnabled: false,
    vadThreshold: params.vadThreshold,
  })

  if (pcmCapture && speakerPart.pcmCapture == null) {
    speakerPart.pcmCapture = pcmCapture
  }

  await Promise.race([
    peerConnected,
    sleepMs(params.connectTimeoutMs).then(() => {
      throw new Error(`timed out waiting for echo host peer connected (${sessionId})`)
    }),
  ])
  await waitForVoiceClientActive(host, peerId, params.connectTimeoutMs)

  return {
    ...speakerPart,
    pcmCapture,
    echoRecorder,
    host,
    cleanup: async () => {
      await speakerPart.cleanup()
      await host.close().catch(() => undefined)
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
  pcmMetrics: MergedHopMetrics | null
  pcmVerdict: PcmFailureVerdict | null
  wavPaths: { out?: string; rx?: string }
}

async function runSpeakerRound(params: {
  session: SpeakerSessionRuntime
  countingPhrase: string
  timeoutMs: number
  finalizeWaitMs: number
  postTtsSilenceS: number
  echoWire?: EchoChildWire
}): Promise<{
  recognized: string
  failures: string[]
  pcmMetrics: PcmHopMetrics | null
  speakerEvents: LifecycleSpeechEvent[]
}> {
  const { session, countingPhrase, timeoutMs, finalizeWaitMs, postTtsSilenceS } = params
  const failures: string[] = []

  session.pcmCapture?.resetRound()
  session.pcmCapture?.startRxCapture()
  session.collector.startEventRecording()

  const echoEndBaseline =
    params.echoWire != null ? await requestEchoEndBaseline(params.echoWire, session.sessionId) : 0

  const playbackPromise = playSpeakerTtsWithPostSilence({
    speaker: session.speaker,
    speakerOut: session.speakerMic,
    phrase: countingPhrase,
    postTtsSilenceS,
    playbackTimeoutMs: DEFAULT_AGENT_TTS_PLAYBACK_TIMEOUT_MS,
    agentSpeakingEndLatch: session.speakerEndLatch,
  })

  const playbackAndEchoDone = playbackPromise.then(async () => {
    if (params.echoWire) {
      await waitEchoAgentSpeakingEnd({
        wire: params.echoWire,
        sessionId: session.sessionId,
        baseline: echoEndBaseline,
        timeoutMs,
      })
    }
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

  const pcmMetrics = session.pcmCapture?.metrics(inboundTranscript) ?? null

  return { recognized: inboundTranscript, failures, pcmMetrics, speakerEvents }
}

async function runFullSessionRound(params: {
  session: FullSessionRuntime
  countingPhrase: string
  timeoutMs: number
  finalizeWaitMs: number
  postTtsSilenceS: number
}): Promise<SessionRoundResult> {
  const { session, countingPhrase, timeoutMs, finalizeWaitMs, postTtsSilenceS } = params

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

  const failures: string[] = []
  const echoAssert = assertFullCountingEcho(inboundTranscript)
  failures.push(...echoAssert.failures.map((f) => `${session.sessionId} inbound: ${f}`))

  const ordering = evaluateEchoLidEventOrdering({
    events: session.echoRecorder.snapshot(),
    label: session.sessionId,
  })
  if (!ordering.passed) {
    failures.push(...ordering.failures)
  }

  const speakerMetrics = session.pcmCapture?.metrics(inboundTranscript) ?? null
  const agentSpeakingStartAt = session.echoRecorder.agentSpeakingStartAt()
  const agentSpeakingEndAt = session.echoRecorder.agentSpeakingEndAt()
  const merged =
    speakerMetrics != null
      ? mergeEchoAndSpeakerMetrics({
          sessionId: session.sessionId,
          recognized: inboundTranscript,
          speaker: speakerMetrics,
          echo: {
            sessionId: session.sessionId,
            agentSpeakingStartAt,
            agentSpeakingEndAt,
            outMs: speakerMetrics.outMs,
            outVoicedMs: speakerMetrics.outVoicedMs,
            out: speakerMetrics.outBurst,
            events: session.echoRecorder.snapshot(),
          },
        })
      : null

  const pcmRealtime = assertEchoOutboundPcmWithinSpeakingWindow({
    sessionId: session.sessionId,
    outMs: merged?.outMs ?? speakerMetrics?.outMs ?? 0,
    agentSpeakingStartAt,
    agentSpeakingEndAt,
  })
  if (!pcmRealtime.ok) {
    failures.push(...pcmRealtime.failures)
  }

  const pcmVerdict =
    merged != null && failures.length > 0
      ? localizeBurstFailure({
          out: merged.out,
          rx: merged.rx,
          recognized: merged.recognized,
          missingEchoPrefix: transcriptMissingEchoPrefix(merged.recognized),
        })
      : null

  return {
    sessionId: session.sessionId,
    recognized: inboundTranscript,
    language: session.echoRecorder.detectedLanguage(),
    orderingOk: ordering.passed,
    failures,
    pcmMetrics: merged,
    pcmVerdict,
    wavPaths: {},
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

function printBurstTables(results: SessionRoundResult[]): void {
  const rows = results.filter((row) => row.pcmMetrics != null)
  if (rows.length === 0) return
  console.log('')
  console.log(
    formatMergedHopTable(
      rows.map((row) => ({
        sessionId: row.sessionId,
        metrics: row.pcmMetrics!,
      })),
    ),
  )
  console.log('')
  console.log(
    formatBurstMetricTable(
      rows.map((row) => ({
        sessionId: row.sessionId,
        out: row.pcmMetrics!.out,
        rx: row.pcmMetrics!.rx,
        partialMinusAgentStartMs: row.pcmMetrics!.partialMinusAgentStartMs,
        recognized: row.pcmMetrics!.recognized,
      })),
    ),
  )

  const failingPcm = results
    .filter((row) => row.failures.length > 0 && row.pcmMetrics != null && row.pcmVerdict != null)
    .map((row) => ({
      sessionId: row.sessionId,
      verdict: row.pcmVerdict!,
      metrics: {
        out: row.pcmMetrics!.out,
        rx: row.pcmMetrics!.rx,
        recognized: row.pcmMetrics!.recognized,
        missingEchoPrefix: transcriptMissingEchoPrefix(row.pcmMetrics!.recognized),
        partialMinusAgentStartMs: row.pcmMetrics!.partialMinusAgentStartMs,
      },
    }))
  if (failingPcm.length > 0) {
    console.log('')
    console.log(formatPcmFailureVerdicts(failingPcm))
    console.log('')
    console.log('=== WAV dumps (failing sessions) ===')
    for (const row of results.filter((r) => r.failures.length > 0)) {
      const paths = [row.wavPaths.out, row.wavPaths.rx].filter(Boolean)
      if (paths.length > 0) {
        console.log(`${row.sessionId}: ${paths.join(', ')}`)
      }
    }
  }
}

async function writeFailureWavs(params: {
  results: SessionRoundResult[]
  sessions: SpeakerSessionRuntime[]
  echoWire: EchoChildWire | null
  wavDir: string | null
}): Promise<void> {
  if (!params.wavDir) return
  const writeAll = process.env.SHERPA_MULTI_WAV_ALL === '1'
  const failing = params.results.filter((r) => writeAll || r.failures.length > 0)
  if (failing.length === 0) return

  mkdirSync(params.wavDir, { recursive: true })

  if (params.echoWire) {
    const echoPaths = await requestEchoWavWrites({
      wire: params.echoWire,
      dir: params.wavDir,
      sessionIds: failing.map((r) => r.sessionId),
    })
    for (const result of failing) {
      result.wavPaths.out = echoPaths.get(result.sessionId)
    }
  }

  for (const result of failing) {
    const session = params.sessions.find((s) => s.sessionId === result.sessionId)
    if (!session?.pcmCapture) continue
    const rxPath = join(params.wavDir, `rx-${result.sessionId}.wav`)
    session.pcmCapture.writeInboundWav(rxPath)
    result.wavPaths.rx = rxPath
    if (!params.echoWire) {
      const outPath = join(params.wavDir, `out-${result.sessionId}.wav`)
      session.pcmCapture.writeOutboundWav(outPath)
      result.wavPaths.out = outPath
    }
  }
}

async function runSplitProcess(): Promise<SessionRoundResult[]> {
  applyPoolEnv(SPEAKER_POOL_ENV)

  const sessionCount = Math.max(1, Math.min(20, Number(process.env.SESSIONS ?? DEFAULT_SESSIONS)))
  if (!process.env.VOICE_MAX_CONCURRENT_SESSIONS) {
    process.env.VOICE_MAX_CONCURRENT_SESSIONS = String(sessionCount + 4)
  }

  const countingPhrase =
    process.env.SHERPA_COUNTING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN
  const { config: base, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()
  const timeoutMs = Number(process.env.SHERPA_COUNTING_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS)
  const finalizeWaitMs = sttFinalizeWaitMs(base)
  const postTtsSilenceS = postTtsSilenceSeconds(base)
  const verbose = process.env.SHERPA_COUNTING_VERBOSE === '1'
  const connectTimeoutMs = Math.min(timeoutMs, 45_000)
  const pcmCaptureEnabled = isMultiPcmCaptureEnabled()
  const vadThreshold = base.vad?.threshold ?? DEFAULT_VAD_ENERGY_THRESHOLD
  const wavDir = resolveMultiWavDir()

  console.log('=== Sherpa 10-session echo + LID (split echo server + speaker clients) ===')
  console.log(`Pipeline: ${label}`)
  console.log(`Sessions: ${sessionCount} concurrent (Promise.all connect + rounds)`)
  console.log(`Topology: split-process (echo child ULTIMATE pool, speaker parent generous pool)`)
  console.log(`SHERPA_MULTI_PCM_CAPTURE=${pcmCaptureEnabled ? '1' : '0'}`)
  console.log(`SHERPA_MULTI_WAV_DIR=${wavDir ?? '(disabled)'}`)
  console.log(
    `Speaker pool: decode=${SPEAKER_POOL_ENV.SHERPA_POOL_MAX_CONCURRENT_DECODE} tts=${SPEAKER_POOL_ENV.SHERPA_POOL_MAX_CONCURRENT_TTS}`,
  )
  console.log(
    `Echo pool (child): decode=${ECHO_POOL_ENV.SHERPA_POOL_MAX_CONCURRENT_DECODE} tts=${ECHO_POOL_ENV.SHERPA_POOL_MAX_CONCURRENT_TTS}`,
  )
  console.log(`STT: ${sttModelPath}`)
  console.log(`TTS: ${ttsModelPath}`)
  console.log(`Speaker phrase: "${countingPhrase}"`)
  console.log('')

  const signalingServer = new SignalingServer({ port: 0 })
  await signalingServer.listen(0)
  const wsUrl = `ws://127.0.0.1:${signalingServer.port}`

  const echoWire = wireEchoServerChild({
    wsUrl,
    sessionCount,
    echoPoolEnv: ECHO_POOL_ENV,
    exportSherpaScript: EXPORT_SHERPA_SCRIPT,
  })
  registerEchoChildKillOnExit(echoWire.child)

  try {
    await waitForEchoHostsListening(echoWire)

    const speakers = await Promise.all(
      Array.from({ length: sessionCount }, (_, index) =>
        setupSpeakerSession({
          sessionIndex: index,
          wsUrl,
          speakerBase: base,
          verbose,
          connectTimeoutMs,
          pcmCaptureEnabled,
          vadThreshold,
        }),
      ),
    )

    await waitForEchoPeersReady(echoWire, connectTimeoutMs)

    sendParentCommand(echoWire.child, { cmd: 'reset-round' })

    const speakerOutcomes = await Promise.all(
      speakers.map((session) =>
        runSpeakerRound({
          session,
          countingPhrase,
          timeoutMs,
          finalizeWaitMs,
          postTtsSilenceS,
          echoWire,
        }),
      ),
    )

    const echoMetrics = await requestEchoMetrics(echoWire)

    const results: SessionRoundResult[] = speakerOutcomes.map((outcome, index) => {
      const sessionId = speakers[index]!.sessionId
      const echo = echoMetrics.find((m) => m.sessionId === sessionId)
      const failures = [...outcome.failures]

      const ordering = evaluateEchoLidEventOrdering({
        events: echo?.events ?? [],
        label: sessionId,
      })
      if (!ordering.passed) {
        failures.push(...ordering.failures)
      }

      const merged =
        outcome.pcmMetrics != null
          ? mergeEchoAndSpeakerMetrics({
              sessionId,
              recognized: outcome.recognized,
              speaker: outcome.pcmMetrics,
              echo,
            })
          : null

      if (merged != null) {
        const pcmRealtime = assertEchoOutboundPcmWithinSpeakingWindow({
          sessionId,
          outMs: merged.outMs,
          agentSpeakingStartAt: merged.agentSpeakingStartAt,
          agentSpeakingEndAt: merged.agentSpeakingEndAt,
        })
        if (!pcmRealtime.ok) {
          failures.push(...pcmRealtime.failures)
        }
      }

      const pcmVerdict =
        merged != null && failures.length > 0
          ? localizeBurstFailure({
              out: merged.out,
              rx: merged.rx,
              recognized: merged.recognized,
              missingEchoPrefix: transcriptMissingEchoPrefix(merged.recognized),
            })
          : null

      return {
        sessionId,
        recognized: outcome.recognized,
        language: languageFromEchoEvents(echo?.events ?? []),
        orderingOk: ordering.passed,
        failures,
        pcmMetrics: merged,
        pcmVerdict,
        wavPaths: {},
      }
    })

    await writeFailureWavs({
      results,
      sessions: speakers,
      echoWire,
      wavDir,
    })

    await Promise.all(speakers.map((s) => s.cleanup().catch(() => undefined)))
    return results
  } finally {
    await shutdownEchoChild(echoWire.child)
    activeEchoChild = null
    await signalingServer.close().catch(() => undefined)
  }
}

async function runSingleProcess(): Promise<SessionRoundResult[]> {
  applyPoolEnv(ECHO_POOL_ENV)

  const sessionCount = Math.max(1, Math.min(20, Number(process.env.SESSIONS ?? DEFAULT_SESSIONS)))
  if (!process.env.VOICE_MAX_CONCURRENT_SESSIONS) {
    process.env.VOICE_MAX_CONCURRENT_SESSIONS = String(sessionCount + 4)
  }

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
  const wavDir = resolveMultiWavDir()

  console.log('=== Sherpa 10-session echo + LID (single-process comparison mode) ===')
  console.log(`Pipeline: ${label}`)
  console.log(`Sessions: ${sessionCount} concurrent (Promise.all connect + rounds)`)
  console.log(`Topology: single-process (SHERPA_MULTI_SINGLE_PROCESS=1)`)
  console.log(`SHERPA_MULTI_PCM_CAPTURE=${pcmCaptureEnabled ? '1' : '0'}`)
  console.log(
    `Pool env: decode=${ECHO_POOL_ENV.SHERPA_POOL_MAX_CONCURRENT_DECODE} tts=${ECHO_POOL_ENV.SHERPA_POOL_MAX_CONCURRENT_TTS}`,
  )
  console.log(`STT: ${sttModelPath}`)
  console.log(`TTS: ${ttsModelPath}`)
  console.log(`LID: ${lidModelPath}`)
  console.log('')

  const signalingServer = new SignalingServer({ port: 0 })
  await signalingServer.listen(0)
  const wsUrl = `ws://127.0.0.1:${signalingServer.port}`

  const sessions = await Promise.all(
    Array.from({ length: sessionCount }, (_, index) =>
      setupFullSession({
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
        runFullSessionRound({
          session,
          countingPhrase,
          timeoutMs,
          finalizeWaitMs,
          postTtsSilenceS,
        }),
      ),
    )

    await writeFailureWavs({
      results,
      sessions,
      echoWire: null,
      wavDir,
    })
  } finally {
    await Promise.all(sessions.map((session) => session.cleanup().catch(() => undefined)))
    await signalingServer.close().catch(() => undefined)
  }

  return results
}

async function main(): Promise<void> {
  enableRoundtripSpeechEventLog()
  installRoundtripWallClockTimeout(Number(process.env.SHERPA_ROUNDTRIP_WALL_MS) || 240_000)

  // Same-host loopback (both processes share the network namespace): do not force
  // WEBRTC_NAT_1TO1_IPS=127.0.0.1 — the host-candidate rewrite breaks ICE inside the CI
  // container (see helpers session-pod-mix-three-client integration test). Browser tabs
  // are not involved here; set the env explicitly if you need it.

  const results = isSingleProcessMode() ? await runSingleProcess() : await runSplitProcess()

  printSummaryTable(results)
  printBurstTables(results)

  const allFailures = results.flatMap((result) => result.failures)
  if (allFailures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: 'concurrent echo + LID multi-session assertions failed',
      failures: allFailures,
      legs: results.map((result) => ({
        label: result.sessionId,
        phrase: process.env.SHERPA_COUNTING_PHRASE?.trim() || DEFAULT_COUNTING_PHRASE_ONE_TO_TEN,
        recognized: result.recognized,
      })),
    })
  }

  console.log(
    `\nConcurrent echo + LID roundtrip OK — ${results.length}/${results.length} sessions full echo one..ten.`,
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
