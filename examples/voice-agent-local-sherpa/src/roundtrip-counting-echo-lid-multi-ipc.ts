/**
 * IPC between multi echo-LID orchestrator (parent) and echo server child.
 */

import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { createInterface } from 'node:readline'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import type {
  PcmBurstMetrics,
  PcmHopMetrics,
} from './roundtrip-counting-echo-lid-multi-pcm-capture.js'
import type { LifecycleSpeechEvent } from './roundtrip-stt-lifecycle-helpers.js'

export const SHERPA_MULTI_JSON_PREFIX = '__SHERPA_MULTI_JSON__'

export interface EchoSessionMetricsPayload {
  sessionId: string
  agentSpeakingStartAt: number | null
  agentSpeakingEndAt: number | null
  outMs: number
  outVoicedMs: number
  out: PcmBurstMetrics
  events: LifecycleSpeechEvent[]
}

export interface EchoMetricsResponse {
  type: 'metrics-response'
  sessions: EchoSessionMetricsPayload[]
}

export interface EchoReadyMessage {
  type: 'ready'
}

export interface EchoWavWrittenMessage {
  type: 'wav-written'
  sessionId: string
  path: string
}

export interface EchoEndBaselineMessage {
  type: 'end-baseline'
  sessionId: string
  count: number
}

export interface EchoAgentSpeakingEndDoneMessage {
  type: 'agent-speaking-end-done'
  sessionId: string
}

export type EchoIpcMessage =
  | EchoReadyMessage
  | EchoMetricsResponse
  | EchoWavWrittenMessage
  | EchoEndBaselineMessage
  | EchoAgentSpeakingEndDoneMessage
  | { type: 'error'; message: string }

export type ParentIpcCommand =
  | { cmd: 'reset-round' }
  | { cmd: 'wait-peers'; timeoutMs?: number }
  | { cmd: 'get-end-baseline'; sessionId: string }
  | { cmd: 'wait-agent-speaking-end'; sessionId: string; baseline: number; timeoutMs?: number }
  | { cmd: 'get-metrics' }
  | { cmd: 'write-wav'; dir: string; sessionIds: string[] }
  | { cmd: 'shutdown' }

const __dirname = dirname(fileURLToPath(import.meta.url))

export function emitEchoIpcMessage(message: EchoIpcMessage): void {
  process.stdout.write(`${SHERPA_MULTI_JSON_PREFIX}${JSON.stringify(message)}\n`)
}

export function sendParentCommand(
  child: ChildProcessWithoutNullStreams,
  command: ParentIpcCommand,
): void {
  child.stdin.write(`${JSON.stringify(command)}\n`)
}

export function prefixChildStream(
  stream: NodeJS.ReadableStream,
  target: NodeJS.WriteStream,
  fallbackLabel: string,
): void {
  const rl = createInterface({ input: stream })
  rl.on('line', (line) => {
    if (line.includes('[voice-debug]')) {
      target.write(`${line}\n`)
      return
    }
    const speechLabel = line.match(/\[speech\] \[(echo-s\d+)\]/)?.[1]
    if (speechLabel) {
      target.write(`[${speechLabel}] ${line}\n`)
      return
    }
    target.write(`[${fallbackLabel}] ${line}\n`)
  })
}

export interface EchoChildWire {
  child: ChildProcessWithoutNullStreams
  waitForMessage: (
    predicate: (msg: EchoIpcMessage) => boolean,
    timeoutMs?: number,
  ) => Promise<EchoIpcMessage>
}

export function wireEchoServerChild(params: {
  wsUrl: string
  sessionCount: number
  echoPoolEnv: Record<string, string>
  exportSherpaScript: string
}): EchoChildWire {
  const echoServerTs = join(__dirname, 'roundtrip-counting-echo-lid-multi-echo-server.ts')
  const child = spawn('bash', [params.exportSherpaScript, '--', 'tsx', echoServerTs], {
    env: {
      ...process.env,
      ...params.echoPoolEnv,
      SHERPA_MULTI_WS_URL: params.wsUrl,
      SESSIONS: String(params.sessionCount),
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  prefixChildStream(child.stderr, process.stderr, 'echo-child')

  const pending: Array<{
    predicate: (msg: EchoIpcMessage) => boolean
    resolve: (msg: EchoIpcMessage) => void
    reject: (error: Error) => void
    deadline: number
  }> = []

  const rl = createInterface({ input: child.stdout })
  rl.on('line', (line) => {
    if (line.startsWith(SHERPA_MULTI_JSON_PREFIX)) {
      try {
        const msg = JSON.parse(line.slice(SHERPA_MULTI_JSON_PREFIX.length)) as EchoIpcMessage
        for (let i = pending.length - 1; i >= 0; i--) {
          const waiter = pending[i]!
          if (waiter.predicate(msg)) {
            pending.splice(i, 1)
            waiter.resolve(msg)
          }
        }
      } catch {
        process.stdout.write(`[echo-child] ${line}\n`)
      }
      return
    }
    process.stdout.write(`[echo-child] ${line}\n`)
  })

  const poll = setInterval(() => {
    const now = Date.now()
    for (let i = pending.length - 1; i >= 0; i--) {
      const waiter = pending[i]!
      if (now > waiter.deadline) {
        pending.splice(i, 1)
        waiter.reject(new Error('timed out waiting for echo IPC message'))
      }
    }
  }, 250)

  child.once('exit', (code, signal) => {
    clearInterval(poll)
    rl.close()
    const err = new Error(
      `echo child exited before IPC ready (code=${code ?? 'null'} signal=${signal ?? 'null'})`,
    )
    for (const waiter of pending.splice(0)) {
      waiter.reject(err)
    }
  })

  return {
    child,
    waitForMessage(predicate, timeoutMs = 30_000) {
      return new Promise((resolve, reject) => {
        pending.push({
          predicate,
          resolve,
          reject,
          deadline: Date.now() + timeoutMs,
        })
      })
    },
  }
}

export async function waitForEchoHostsListening(
  wire: EchoChildWire,
  timeoutMs = 120_000,
): Promise<void> {
  const msg = await wire.waitForMessage((m) => m.type === 'ready' || m.type === 'error', timeoutMs)
  if (msg.type === 'error') {
    throw new Error(msg.message)
  }
}

export async function waitForEchoPeersReady(
  wire: EchoChildWire,
  timeoutMs = 45_000,
): Promise<void> {
  sendParentCommand(wire.child, { cmd: 'wait-peers', timeoutMs })
  const msg = await wire.waitForMessage((m) => m.type === 'ready' || m.type === 'error', timeoutMs)
  if (msg.type === 'error') {
    throw new Error(msg.message)
  }
}

export async function requestEchoMetrics(
  wire: EchoChildWire,
  timeoutMs = 30_000,
): Promise<EchoSessionMetricsPayload[]> {
  sendParentCommand(wire.child, { cmd: 'get-metrics' })
  const msg = await wire.waitForMessage(
    (m) => m.type === 'metrics-response' || m.type === 'error',
    timeoutMs,
  )
  if (msg.type === 'error') {
    throw new Error(msg.message)
  }
  if (msg.type !== 'metrics-response') {
    throw new Error(`unexpected echo IPC message: ${msg.type}`)
  }
  return msg.sessions
}

export async function requestEchoEndBaseline(
  wire: EchoChildWire,
  sessionId: string,
  timeoutMs = 10_000,
): Promise<number> {
  sendParentCommand(wire.child, { cmd: 'get-end-baseline', sessionId })
  const msg = await wire.waitForMessage(
    (m) => (m.type === 'end-baseline' && m.sessionId === sessionId) || m.type === 'error',
    timeoutMs,
  )
  if (msg.type === 'error') {
    throw new Error(msg.message)
  }
  if (msg.type !== 'end-baseline') {
    throw new Error(`unexpected echo IPC message: ${msg.type}`)
  }
  return msg.count
}

export async function waitEchoAgentSpeakingEnd(params: {
  wire: EchoChildWire
  sessionId: string
  baseline: number
  timeoutMs?: number
}): Promise<void> {
  sendParentCommand(params.wire.child, {
    cmd: 'wait-agent-speaking-end',
    sessionId: params.sessionId,
    baseline: params.baseline,
    timeoutMs: params.timeoutMs,
  })
  const msg = await params.wire.waitForMessage(
    (m) =>
      (m.type === 'agent-speaking-end-done' && m.sessionId === params.sessionId) ||
      m.type === 'error',
    params.timeoutMs ?? 90_000,
  )
  if (msg.type === 'error') {
    throw new Error(msg.message)
  }
}

export async function requestEchoWavWrites(params: {
  wire: EchoChildWire
  dir: string
  sessionIds: string[]
  timeoutMs?: number
}): Promise<Map<string, string>> {
  const paths = new Map<string, string>()
  sendParentCommand(params.wire.child, {
    cmd: 'write-wav',
    dir: params.dir,
    sessionIds: params.sessionIds,
  })

  for (const sessionId of params.sessionIds) {
    const msg = await params.wire.waitForMessage(
      (m) => (m.type === 'wav-written' && m.sessionId === sessionId) || m.type === 'error',
      params.timeoutMs ?? 30_000,
    )
    if (msg.type === 'error') {
      throw new Error(msg.message)
    }
    if (msg.type !== 'wav-written') {
      throw new Error(`unexpected echo IPC message: ${msg.type}`)
    }
    paths.set(msg.sessionId, msg.path)
  }
  return paths
}

export function shutdownEchoChild(child: ChildProcessWithoutNullStreams): Promise<void> {
  return new Promise((resolve) => {
    sendParentCommand(child, { cmd: 'shutdown' })
    const timer = setTimeout(() => {
      child.kill('SIGTERM')
      resolve()
    }, 5000)
    child.once('exit', () => {
      clearTimeout(timer)
      resolve()
    })
  })
}

export interface MergedHopMetrics extends PcmHopMetrics {
  out: PcmBurstMetrics
  rx: PcmBurstMetrics
  agentSpeakingStartAt: number | null
  agentSpeakingEndAt: number | null
  speakingWallMs: number | null
  outMsRatio: number | null
  partialMinusAgentStartMs: number | null
}

export function mergeEchoAndSpeakerMetrics(params: {
  sessionId: string
  recognized: string
  speaker: PcmHopMetrics
  echo: EchoSessionMetricsPayload | undefined
}): MergedHopMetrics {
  const emptyBurst: PcmBurstMetrics = {
    firstBurstMs: 0,
    gapAfterFirstBurstMs: 0,
    burstCount: 0,
    totalVoicedMs: 0,
  }
  const agentSpeakingStartAt = params.echo?.agentSpeakingStartAt ?? null
  const agentSpeakingEndAt = params.echo?.agentSpeakingEndAt ?? null
  const outMs = params.echo?.outMs ?? params.speaker.outMs
  const speakingWallMs =
    agentSpeakingStartAt != null && agentSpeakingEndAt != null
      ? agentSpeakingEndAt - agentSpeakingStartAt
      : null
  const outMsRatio = speakingWallMs != null && speakingWallMs > 0 ? outMs / speakingWallMs : null
  const firstPartialAt = params.speaker.firstPartialAt
  const partialMinusAgentStartMs =
    firstPartialAt != null && agentSpeakingStartAt != null
      ? firstPartialAt - agentSpeakingStartAt
      : null

  return {
    ...params.speaker,
    recognized: params.recognized,
    outMs,
    outVoicedMs: params.echo?.outVoicedMs ?? params.speaker.outVoicedMs,
    outBurst: params.echo?.out ?? params.speaker.outBurst,
    out: params.echo?.out ?? emptyBurst,
    rx: params.speaker.rxBurst,
    agentSpeakingStartAt,
    agentSpeakingEndAt,
    speakingWallMs,
    outMsRatio,
    partialMinusAgentStartMs,
  }
}
