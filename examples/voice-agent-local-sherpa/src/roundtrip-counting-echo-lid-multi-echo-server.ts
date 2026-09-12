/**
 * Echo-server child for multi echo-LID roundtrip — VoiceAgentSessionHost echo agents.
 *
 * Spawned by roundtrip-counting-echo-lid-multi.ts (parent holds signaling + speaker clients).
 * Ultimate-tier Sherpa pool env is applied to this process only (see parent spawn env).
 *
 * Env:
 *   SHERPA_MULTI_WS_URL   ws://127.0.0.1:<port> (required)
 *   SESSIONS              echo agent count (default 10, max 20)
 *   VOICE_DEBUG           forwarded from parent for Rust [voice-debug] on stderr
 */

import { createInterface } from 'node:readline'
import { join } from 'node:path'

import {
  VOICE_AGENT_SERVER_PEER_ID,
  VoiceAgentSessionHost,
  VoiceSessionBudget,
  type VoiceSessionHandler,
} from '@node-webrtc-rust/helpers'
import { SignalingClient } from '@node-webrtc-rust/signaling'
import {
  SPEECH_EVENT_TYPE,
  type SpeechEvent,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

import { DEMO_ICE_SERVERS } from '../../shared/webrtc-demo-helpers.js'
import { resolveRoundtripVoiceConfig, withRoundtripHarnessSilence } from './resolve-voice-config.js'
import { AgentSpeakingEndLatch } from './roundtrip-counting.js'
import { echoVadConfig } from './roundtrip-counting-echo.js'
import {
  CLOUD_DEFAULT_LID_MIN_SPEECH_MS,
  formatEchoSmokeReply,
} from './roundtrip-counting-echo-lid-prefix.js'
import {
  emitEchoIpcMessage,
  type ParentIpcCommand,
} from './roundtrip-counting-echo-lid-multi-ipc.js'
import {
  MultiSessionPcmCapture,
  wrapOutboundTrackForPcmCapture,
} from './roundtrip-counting-echo-lid-multi-pcm-capture.js'
import { resolveLidModelPath } from './roundtrip-language-id.js'
import { enableSherpaRoundtripRustDebug } from './roundtrip-failure-debug.js'
import {
  enableRoundtripSpeechEventLog,
  logRoundtripSpeechEvent,
} from './roundtrip-speech-events.js'
import type { LifecycleSpeechEvent } from './roundtrip-stt-lifecycle-helpers.js'

const DEFAULT_SESSIONS = 10
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

interface EchoSessionRuntime {
  sessionId: string
  pcmCapture: MultiSessionPcmCapture
  echoRecorder: EchoSpeechEventRecorder
  host: VoiceAgentSessionHost
  cleanup: () => Promise<void>
}

class EchoSpeechEventRecorder {
  private events: LifecycleSpeechEvent[] = []
  private t0 = Date.now()
  private endLatch = new AgentSpeakingEndLatch()

  constructor(
    private readonly label: string,
    private readonly onEvent?: (event: SpeechEvent, atMs: number) => void,
  ) {}

  reset(): void {
    this.events = []
    this.t0 = Date.now()
    this.endLatch = new AgentSpeakingEndLatch()
  }

  observe(event: SpeechEvent): void {
    this.endLatch.observe(event)
    logRoundtripSpeechEvent(this.label, event)
    const atMs = Date.now() - this.t0
    this.events.push({
      type: event.type,
      atMs,
      text: event.text,
    })
    this.onEvent?.(event, atMs)
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

  snapshot(): LifecycleSpeechEvent[] {
    return [...this.events]
  }

  endEventsSeen(): number {
    return this.endLatch.endEventsSeen()
  }

  waitAfterEndCount(baseline: number, timeoutMs: number): Promise<void> {
    return this.endLatch.waitAfterCount(baseline, timeoutMs)
  }
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

async function setupEchoSession(params: {
  sessionIndex: number
  wsUrl: string
  echoConfig: VoiceAgentConfig
  sessionBudget: VoiceSessionBudget
  vadThreshold: number
  connectTimeoutMs: number
}): Promise<EchoSessionRuntime> {
  const sessionId = `s${params.sessionIndex + 1}`
  const room = `lid-echo-multi-${sessionId}`
  const pcmCapture = new MultiSessionPcmCapture(params.vadThreshold)
  let replyCapturePending = false

  const echoRecorder = new EchoSpeechEventRecorder(`echo-${sessionId}`, (event, atMs) => {
    if (event.type === SPEECH_EVENT_TYPE.userSpeechFinal) {
      replyCapturePending = true
    }
    if (replyCapturePending && event.type === SPEECH_EVENT_TYPE.agentSpeakingStart) {
      replyCapturePending = false
      pcmCapture.startEchoOutboundCapture()
    }
    if (event.type === SPEECH_EVENT_TYPE.agentSpeakingStart) {
      pcmCapture.observeAgentSpeakingStart(atMs)
    }
    if (event.type === SPEECH_EVENT_TYPE.agentSpeakingEnd) {
      pcmCapture.stopEchoOutboundCapture()
    }
  })

  const voiceHandler: VoiceSessionHandler = {
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
    wrapAudioTracks: ({ outbound }) => ({
      outbound: wrapOutboundTrackForPcmCapture(outbound, pcmCapture),
    }),
  })

  return {
    sessionId,
    pcmCapture,
    echoRecorder,
    host,
    cleanup: async () => {
      await host.close().catch(() => undefined)
      serverSignaling.disconnect()
    },
  }
}

async function main(): Promise<void> {
  enableRoundtripSpeechEventLog()
  enableSherpaRoundtripRustDebug()

  const wsUrl = process.env.SHERPA_MULTI_WS_URL
  if (!wsUrl) {
    emitEchoIpcMessage({ type: 'error', message: 'SHERPA_MULTI_WS_URL is required' })
    process.exit(1)
  }

  const sessionCount = Math.max(1, Math.min(20, Number(process.env.SESSIONS ?? DEFAULT_SESSIONS)))
  if (!process.env.VOICE_MAX_CONCURRENT_SESSIONS) {
    process.env.VOICE_MAX_CONCURRENT_SESSIONS = String(sessionCount + 4)
  }

  const { config: base } = resolveRoundtripVoiceConfig()
  const lidModelPath = resolveLidModelPath()
  const echoConfig = echoHostConfig(base, lidModelPath)
  const sessionBudget = new VoiceSessionBudget(0)
  const vadThreshold = base.vad?.threshold ?? 0.15
  const connectTimeoutMs = 45_000

  const sessions = await Promise.all(
    Array.from({ length: sessionCount }, (_, index) =>
      setupEchoSession({
        sessionIndex: index,
        wsUrl,
        echoConfig,
        sessionBudget,
        vadThreshold,
        connectTimeoutMs,
      }),
    ),
  )

  emitEchoIpcMessage({ type: 'ready' })

  const rl = createInterface({ input: process.stdin })
  rl.on('line', (line) => {
    void handleCommand(line.trim(), sessions).catch((error: unknown) => {
      emitEchoIpcMessage({
        type: 'error',
        message: error instanceof Error ? error.message : String(error),
      })
    })
  })
}

async function handleCommand(line: string, sessions: EchoSessionRuntime[]): Promise<void> {
  if (!line) return
  const command = JSON.parse(line) as ParentIpcCommand

  if (command.cmd === 'reset-round') {
    for (const session of sessions) {
      session.echoRecorder.reset()
      session.pcmCapture.resetRound()
    }
    return
  }

  if (command.cmd === 'wait-peers') {
    const timeoutMs = command.timeoutMs ?? 45_000
    for (const session of sessions) {
      const peerId = `client-${session.sessionId}`
      await waitForVoiceClientActive(session.host, peerId, timeoutMs)
    }
    emitEchoIpcMessage({ type: 'ready' })
    return
  }

  if (command.cmd === 'get-end-baseline') {
    const session = sessions.find((s) => s.sessionId === command.sessionId)
    if (!session) {
      throw new Error(`unknown echo session ${command.sessionId}`)
    }
    emitEchoIpcMessage({
      type: 'end-baseline',
      sessionId: command.sessionId,
      count: session.echoRecorder.endEventsSeen(),
    })
    return
  }

  if (command.cmd === 'wait-agent-speaking-end') {
    const session = sessions.find((s) => s.sessionId === command.sessionId)
    if (!session) {
      throw new Error(`unknown echo session ${command.sessionId}`)
    }
    await session.echoRecorder.waitAfterEndCount(command.baseline, command.timeoutMs ?? 90_000)
    emitEchoIpcMessage({ type: 'agent-speaking-end-done', sessionId: command.sessionId })
    return
  }

  if (command.cmd === 'get-metrics') {
    emitEchoIpcMessage({
      type: 'metrics-response',
      sessions: sessions.map((session) => ({
        sessionId: session.sessionId,
        agentSpeakingStartAt: session.echoRecorder.agentSpeakingStartAt(),
        agentSpeakingEndAt: session.echoRecorder.agentSpeakingEndAt(),
        outMs: session.pcmCapture.outMs,
        outVoicedMs: session.pcmCapture.outVoicedMs,
        out: session.pcmCapture.outBurstMetrics(),
        events: session.echoRecorder.snapshot(),
      })),
    })
    return
  }

  if (command.cmd === 'write-wav') {
    for (const sessionId of command.sessionIds) {
      const session = sessions.find((s) => s.sessionId === sessionId)
      if (!session) continue
      const path = join(command.dir, `out-${sessionId}.wav`)
      session.pcmCapture.writeOutboundWav(path)
      emitEchoIpcMessage({ type: 'wav-written', sessionId, path })
    }
    return
  }

  if (command.cmd === 'shutdown') {
    await Promise.all(sessions.map((session) => session.cleanup().catch(() => undefined)))
    process.exit(0)
  }
}

const isMain =
  process.argv[1]?.endsWith('roundtrip-counting-echo-lid-multi-echo-server.ts') === true

if (isMain) {
  main().catch((error: unknown) => {
    emitEchoIpcMessage({
      type: 'error',
      message: error instanceof Error ? error.message : String(error),
    })
    process.exit(1)
  })
}
