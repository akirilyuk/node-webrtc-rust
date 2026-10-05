/**
 * B2 E2E: a barge-in must not poison the TTS phrase cache (Sherpa STT + TTS, full stack).
 *
 * A cancelled synthesis used to be stored in the phrase cache under the full text, so the next
 * identical phrase played truncated. This roundtrip speaks a multi-sentence phrase that was
 * never spoken in this process (a nonce makes it unique), barges in mid-way, then speaks the same
 * phrase again without a barge.
 *
 * d(agent) = agent_speaking_start → agent_speaking_end for one `sendTextToTTS` call.
 *   Phase R: separate agent, own cache scope (`ref-<nonce>`), phrase P, no barge → dRef
 *   Phase A: main agent (`p1-<nonce>`) speaks P, user leg says "stop now please" 1500 ms after
 *            agent_speaking_start → barge_in → dA
 *   Phase B: main agent speaks P again, no barge → dB
 * Pass: dA < 0.5 x dRef (the barge happened) and dB >= 0.85 x dRef (the replay is complete).
 *
 * Run:
 *   npm run start:roundtrip-barge-replay --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * See ROUNDTRIP.md § Barge-in replay (B2).
 */

import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  SPEECH_EVENT_TYPE,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import {
  agentSpeakingDurationMs,
  evaluateBargeReplayDurations,
  formatRecordedSpeechEvent,
  phase2EventsComplete,
  phase3EventsTerminal,
  recordSpeechEvent,
  type RecordedSpeechEvent,
} from './roundtrip-barge-in-helpers.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import {
  installRoundtripWallClockTimeout,
  interPhaseSttDrainSeconds,
  postTtsSilenceSeconds,
} from './roundtrip-counting.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import {
  attachRoundtripConnectionLogs,
  logE2ePhase,
  logRoundtripScriptBanner,
  logSignalingReady,
  logVoiceAgentAttach,
} from './roundtrip-topology-log.js'

const FOUR_SENTENCES =
  'The first sentence is about the weather today. ' +
  'The second sentence talks about the morning trains. ' +
  'The third sentence mentions a small cafe near the station. ' +
  'The fourth sentence closes the reply politely.'

const BARGE_PHRASE = 'stop now please'
const BARGE_DELAY_MS = 1500
const MAX_PHASE_MS = 40_000
const WARMUP_S = 0.6

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function listenerVadConfig(base: VoiceAgentConfig): NonNullable<VoiceAgentConfig['vad']> {
  return {
    ...VOICE_AGENT_VAD_PRESET,
    ...base.vad,
    provider: 'energy',
    threshold: 0.05,
    gateStt: true,
    bargeIn: {
      ...VOICE_AGENT_VAD_PRESET.bargeIn,
      ...base.vad?.bargeIn,
      enabled: true,
      useVad: true,
      flushTts: true,
      requireSttPartial: true,
      agentPlaybackGuardMs: 0,
    },
  }
}

/** One `speechEvents()` pump per agent for the whole run; phases slice by index. */
class AgentEventLog {
  readonly events: RecordedSpeechEvent[] = []
  private readonly startedAtMs = Date.now()
  private waiters: Array<() => void> = []

  constructor(
    private readonly label: string,
    agent: VoiceAgent,
  ) {
    void this.pump(agent)
  }

  mark(): number {
    return this.events.length
  }

  since(mark: number): RecordedSpeechEvent[] {
    return this.events.slice(mark)
  }

  /** Resolve when `until(events since mark)` holds, or after `maxMs` (returns what was seen). */
  async waitUntil(
    mark: number,
    until: (events: RecordedSpeechEvent[]) => boolean,
    maxMs: number,
  ): Promise<RecordedSpeechEvent[]> {
    const deadline = Date.now() + maxMs
    while (!until(this.since(mark))) {
      const left = deadline - Date.now()
      if (left <= 0) {
        console.error(`[${this.label}] wall-clock cap ${maxMs} ms while waiting for events`)
        break
      }
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, Math.min(left, 250))
        this.waiters.push(() => {
          clearTimeout(timer)
          resolve()
        })
      })
    }
    return this.since(mark)
  }

  private async pump(agent: VoiceAgent): Promise<void> {
    try {
      for await (const event of agent.speechEvents()) {
        const recorded = recordSpeechEvent(this.events, event, this.startedAtMs)
        console.log(`[${this.label}] event ${formatRecordedSpeechEvent(recorded)}`)
        const waiters = this.waiters
        this.waiters = []
        for (const wake of waiters) wake()
      }
    } catch (error) {
      console.error(`[${this.label}] speech-events stream error:`, error)
    }
  }
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(150_000)

  const nonce = Date.now()
  const phrase = `${FOUR_SENTENCES} Run ${nonce}.`
  const { config, label, sttModelPath, ttsModelPath } = resolveRoundtripVoiceConfig()

  console.log('=== Sherpa barge-in replay E2E (B2: cancelled synthesis must not be cached) ===')
  logRoundtripScriptBanner({
    script: 'roundtrip-barge-replay',
    pipeline: label,
    extra: [`nonce=${nonce}`, `bargeDelayMs=${BARGE_DELAY_MS}`, `maxPhaseMs=${MAX_PHASE_MS}`],
  })
  console.log(`Pipeline: ${label}`)
  console.log(`Phrase (${phrase.length} chars): ${phrase}`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('')

  const { server, agentPc, userPc, agentOut, userOut, userInbound, agentInbound, cleanup } =
    await createBidirectionalLoopback()
  logSignalingReady({ port: server.port })
  attachRoundtripConnectionLogs({ agentPc, userPc })

  // Main agent: STT + VAD + barge on agent-pc, plays phrase P in phases A and B.
  const listener = new VoiceAgent({
    stt: config.stt,
    tts: config.tts,
    events: { mode: 'stream' },
    vad: listenerVadConfig(config),
  })
  // User leg: TTS only. Speaks the barge phrase, and acts as the separate reference agent
  // (own cache scope) in phase R.
  const userSpeaker = new VoiceAgent({
    tts: config.tts,
    events: { mode: 'stream' },
    vad: { enabled: false },
  })

  await listener.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  logVoiceAgentAttach({
    role: 'listener',
    label: 'main VoiceAgent (STT+VAD+barge, speaks P) on agent-pc',
    inboundTrack: 'agentInbound ← user-pc RTP',
    outboundTrack: 'agentOut → user-pc',
  })
  await userSpeaker.attach({ inboundTrack: userInbound, outboundTrack: userOut })
  logVoiceAgentAttach({
    role: 'user-sim',
    label: 'user-leg VoiceAgent (TTS only; reference speaker + barge phrase) on user-pc',
    inboundTrack: 'userInbound ← agent-pc RTP',
    outboundTrack: 'userOut → agent-pc',
  })
  await listener.start({ projectId: `p1-${nonce}` })
  await userSpeaker.start({ projectId: `ref-${nonce}` })

  const listenerLog = new AgentEventLog('main', listener)
  const userLog = new AgentEventLog('user-leg', userSpeaker)

  await streamSilence(agentOut, WARMUP_S)
  await streamSilence(userOut, WARMUP_S)

  const drainS = interPhaseSttDrainSeconds(config)
  const postSilenceS = postTtsSilenceSeconds(config)
  const failures: string[] = []

  // Phase R: reference duration, own cache scope, no barge.
  logE2ePhase({ phase: 'Phase R', detail: 'reference agent speaks P (no barge)' })
  console.log('--- Phase R: reference playback (scope ref-<nonce>, no barge) ---')
  const markR = userLog.mark()
  const refSpeak = userSpeaker.sendTextToTTS(phrase)
  const refEvents = await userLog.waitUntil(markR, phase2EventsComplete, MAX_PHASE_MS)
  await refSpeak
  const dRef = agentSpeakingDurationMs(refEvents)
  console.log(`[Phase R] dRef=${dRef} ms`)
  await streamSilence(agentOut, 0.3)
  await streamSilence(userOut, drainS)

  // Phase A: main agent speaks P, user leg barges in.
  logE2ePhase({ phase: 'Phase A', detail: `main agent speaks P, barge at +${BARGE_DELAY_MS} ms` })
  console.log('--- Phase A: main agent speaks P, user leg barges in ---')
  const markA = listenerLog.mark()
  const speakA = listener.sendTextToTTS(phrase)
  await listenerLog.waitUntil(
    markA,
    (events) => events.some((e) => e.type === SPEECH_EVENT_TYPE.agentSpeakingStart),
    MAX_PHASE_MS,
  )
  await sleep(BARGE_DELAY_MS)
  console.log(`[Phase A] user barge: "${BARGE_PHRASE}"`)
  await userSpeaker.sendTextToTTS(BARGE_PHRASE)
  void streamSilence(userOut, postSilenceS)
  const eventsA = await listenerLog.waitUntil(markA, phase3EventsTerminal, MAX_PHASE_MS)
  await speakA
  const dA = agentSpeakingDurationMs(eventsA)
  const barges = eventsA.filter((e) => e.type === SPEECH_EVENT_TYPE.bargeIn).length
  console.log(`[Phase A] dA=${dA} ms barge_in events=${barges}`)
  if (barges === 0) {
    failures.push('Phase A: no barge_in event, so the cancel path was never exercised')
  }
  await streamSilence(agentOut, 0.3)
  await streamSilence(userOut, drainS)

  // Phase B: same phrase, same scope, no barge.
  logE2ePhase({ phase: 'Phase B', detail: 'main agent speaks P again (no barge)' })
  console.log('--- Phase B: main agent replays P (no barge) ---')
  const markB = listenerLog.mark()
  const speakB = listener.sendTextToTTS(phrase)
  const eventsB = await listenerLog.waitUntil(markB, phase2EventsComplete, MAX_PHASE_MS)
  await speakB
  const dB = agentSpeakingDurationMs(eventsB)
  console.log(`[Phase B] dB=${dB} ms`)

  const evaluation = evaluateBargeReplayDurations({
    dRefMs: dRef,
    dBargedMs: dA,
    dReplayMs: dB,
  })
  console.log(`Barge replay: ${evaluation.summary}`)
  failures.push(...evaluation.failures)

  await listener.stop().catch(() => undefined)
  await userSpeaker.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({
      reason: 'barge-in replay assertions failed',
      failures,
    })
  }

  console.log('\nBarge-in replay E2E OK — the replay after a barge played the full phrase.')
  process.exit(0)
}

main().catch((error: unknown) => {
  exitSherpaRoundtripFailure({
    reason: 'uncaught error',
    error,
  })
})
