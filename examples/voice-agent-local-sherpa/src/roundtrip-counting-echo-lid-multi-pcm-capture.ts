/**
 * Per-hop PCM metrics for multi echo-LID roundtrip (harness only — no Rust changes).
 *
 * Hops:
 *   (a) echo outbound — PCM written to the echo agent's local TTS track
 *   (b) speaker received — PCM from RemoteAudioTrack.readSample on the speaker leg
 *   (c) speaker STT-fed — user_speaking_start / vad_triggered vs first non-silent RX
 */

import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'

import { stereoPcmDurationMs } from './pcm-relay.js'
import { transcriptHasPrefixBeforeCounting } from './roundtrip-counting-echo-lid-prefix.js'

export const DEFAULT_VAD_ENERGY_THRESHOLD = 0.15
export const SILENCE_GAP_MS = 120

export function isMultiPcmCaptureEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const raw = env.SHERPA_MULTI_PCM_CAPTURE
  if (raw === undefined || raw === '') return true
  return raw !== '0' && raw.toLowerCase() !== 'false'
}

export function resolveMultiWavDir(env: NodeJS.ProcessEnv = process.env): string | null {
  const raw = env.SHERPA_MULTI_WAV_DIR
  if (raw === undefined || raw === '') {
    const stamp = new Date().toISOString().replace(/[:.]/g, '-')
    return join('.test-logs', 'multi-wav', stamp)
  }
  return raw
}

function stereoPeakRmsNormalized(pcm: Buffer): number {
  const pairs = Math.floor(pcm.byteLength / 4)
  if (pairs === 0) return 0
  let sum = 0
  for (let i = 0; i < pairs; i++) {
    const l = pcm.readInt16LE(i * 4)
    const r = pcm.readInt16LE(i * 4 + 2)
    const peak = Math.max(Math.abs(l), Math.abs(r))
    sum += peak * peak
  }
  return Math.sqrt(sum / pairs) / 32768
}

function isVoicedPcm(pcm: Buffer, threshold: number): boolean {
  return stereoPeakRmsNormalized(pcm) >= threshold
}

export interface PcmBurstMetrics {
  firstBurstMs: number
  gapAfterFirstBurstMs: number
  burstCount: number
  totalVoicedMs: number
}

interface VoicedFrame {
  durationMs: number
  voiced: boolean
}

/** Silence between bursts counts only when ≥120 ms below threshold. */
export function computeBurstMetrics(frames: VoicedFrame[]): PcmBurstMetrics {
  if (frames.length === 0) {
    return { firstBurstMs: 0, gapAfterFirstBurstMs: 0, burstCount: 0, totalVoicedMs: 0 }
  }

  const bursts: Array<{ voicedMs: number; gapAfterMs: number }> = []
  let currentVoicedMs = 0
  let silenceMs = 0
  let inBurst = false

  for (const frame of frames) {
    if (frame.voiced) {
      if (!inBurst) {
        if (silenceMs >= SILENCE_GAP_MS || bursts.length === 0) {
          inBurst = true
          currentVoicedMs = 0
        } else {
          // short silence inside a burst — keep accumulating voiced
          inBurst = true
        }
        silenceMs = 0
      }
      currentVoicedMs += frame.durationMs
    } else {
      silenceMs += frame.durationMs
      if (inBurst && silenceMs >= SILENCE_GAP_MS) {
        bursts.push({ voicedMs: currentVoicedMs, gapAfterMs: silenceMs })
        inBurst = false
        currentVoicedMs = 0
      }
    }
  }

  if (inBurst && currentVoicedMs > 0) {
    bursts.push({ voicedMs: currentVoicedMs, gapAfterMs: 0 })
  }

  const totalVoicedMs = frames.filter((f) => f.voiced).reduce((sum, f) => sum + f.durationMs, 0)

  return {
    firstBurstMs: bursts[0]?.voicedMs ?? 0,
    gapAfterFirstBurstMs: bursts[0]?.gapAfterMs ?? 0,
    burstCount: bursts.length,
    totalVoicedMs,
  }
}

export interface PcmHopMetrics {
  outMs: number
  outVoicedMs: number
  rxMs: number
  rxVoicedMs: number
  rxReadCount: number
  maxRxGapMs: number
  firstRxNonSilentAt: number | null
  agentSpeakingStartAt: number | null
  userSpeakingStartAt: number | null
  vadTriggeredAt: number | null
  firstPartialAt: number | null
  recognized: string
  rxBurst: PcmBurstMetrics
  outBurst: PcmBurstMetrics
}

export type PcmFailureVerdict =
  | 'sender: stretched inter-sentence gap under TTS pool contention'
  | 'transport/pacing'
  | 'receiver STT gating/decode'
  | 'unknown'

export class MultiSessionPcmCapture {
  private roundT0 = performance.now()
  private echoOutActive = false
  private rxActive = false
  private lastRxResolvedAt: number | null = null

  private outFrames: VoicedFrame[] = []
  private rxFrames: VoicedFrame[] = []
  private outPcmChunks: Buffer[] = []
  private rxPcmChunks: Buffer[] = []

  outMs = 0
  outVoicedMs = 0
  rxMs = 0
  rxVoicedMs = 0
  rxReadCount = 0
  maxRxGapMs = 0
  firstRxNonSilentAt: number | null = null
  userSpeakingStartAt: number | null = null
  vadTriggeredAt: number | null = null
  firstPartialAt: number | null = null
  agentSpeakingStartAt: number | null = null

  constructor(private readonly vadThreshold = DEFAULT_VAD_ENERGY_THRESHOLD) {}

  resetRound(): void {
    this.roundT0 = performance.now()
    this.echoOutActive = false
    this.rxActive = false
    this.lastRxResolvedAt = null
    this.outFrames = []
    this.rxFrames = []
    this.outPcmChunks = []
    this.rxPcmChunks = []
    this.outMs = 0
    this.outVoicedMs = 0
    this.rxMs = 0
    this.rxVoicedMs = 0
    this.rxReadCount = 0
    this.maxRxGapMs = 0
    this.firstRxNonSilentAt = null
    this.userSpeakingStartAt = null
    this.vadTriggeredAt = null
    this.firstPartialAt = null
    this.agentSpeakingStartAt = null
  }

  startEchoOutboundCapture(): void {
    this.echoOutActive = true
  }

  stopEchoOutboundCapture(): void {
    this.echoOutActive = false
  }

  startRxCapture(): void {
    this.rxActive = true
    this.lastRxResolvedAt = null
  }

  stopRxCapture(): void {
    this.rxActive = false
  }

  observeAgentSpeakingStart(atRoundMs: number): void {
    if (this.agentSpeakingStartAt == null) {
      this.agentSpeakingStartAt = atRoundMs
    }
  }

  recordOutboundPcm(pcm: Buffer): void {
    if (!this.echoOutActive || pcm.length === 0) return
    const durationMs = stereoPcmDurationMs(pcm.length)
    const voiced = isVoicedPcm(pcm, this.vadThreshold)
    this.outFrames.push({ durationMs, voiced })
    this.outPcmChunks.push(Buffer.from(pcm))
    this.outMs += durationMs
    if (voiced) {
      this.outVoicedMs += durationMs
    }
  }

  recordInboundRead(pcm: Buffer, resolvedAtMs: number): void {
    if (!this.rxActive || pcm.length === 0) return
    const durationMs = stereoPcmDurationMs(pcm.length)
    const voiced = isVoicedPcm(pcm, this.vadThreshold)
    this.rxFrames.push({ durationMs, voiced })
    this.rxPcmChunks.push(Buffer.from(pcm))
    this.rxMs += durationMs
    this.rxReadCount += 1

    if (this.lastRxResolvedAt != null) {
      const gap = resolvedAtMs - this.lastRxResolvedAt
      if (gap > this.maxRxGapMs) {
        this.maxRxGapMs = gap
      }
    }
    this.lastRxResolvedAt = resolvedAtMs

    if (voiced) {
      this.rxVoicedMs += durationMs
      if (this.firstRxNonSilentAt == null) {
        this.firstRxNonSilentAt = resolvedAtMs - this.roundT0
      }
    }
  }

  observeSpeakerSpeechEvent(type: string, atRoundMs: number): void {
    if (type === 'user_speaking_start' && this.userSpeakingStartAt == null) {
      this.userSpeakingStartAt = atRoundMs
    }
    if (type === 'vad_triggered' && this.vadTriggeredAt == null) {
      this.vadTriggeredAt = atRoundMs
    }
    if (type === 'user_speech_partial' && this.firstPartialAt == null) {
      this.firstPartialAt = atRoundMs
    }
    if (type === 'agent_speaking_start') {
      this.observeAgentSpeakingStart(atRoundMs)
    }
  }

  outBurstMetrics(): PcmBurstMetrics {
    return computeBurstMetrics(this.outFrames)
  }

  rxBurstMetrics(): PcmBurstMetrics {
    return computeBurstMetrics(this.rxFrames)
  }

  metrics(recognized: string): PcmHopMetrics {
    return {
      outMs: this.outMs,
      outVoicedMs: this.outVoicedMs,
      rxMs: this.rxMs,
      rxVoicedMs: this.rxVoicedMs,
      rxReadCount: this.rxReadCount,
      maxRxGapMs: this.maxRxGapMs,
      firstRxNonSilentAt: this.firstRxNonSilentAt,
      agentSpeakingStartAt: this.agentSpeakingStartAt,
      userSpeakingStartAt: this.userSpeakingStartAt,
      vadTriggeredAt: this.vadTriggeredAt,
      firstPartialAt: this.firstPartialAt,
      recognized,
      outBurst: this.outBurstMetrics(),
      rxBurst: this.rxBurstMetrics(),
    }
  }

  writeOutboundWav(path: string): void {
    writeMonoWav16k(path, stereo48kToMono16k(this.outPcmChunks))
  }

  writeInboundWav(path: string): void {
    writeMonoWav16k(path, stereo48kToMono16k(this.rxPcmChunks))
  }
}

/** Downsample interleaved stereo 48 kHz s16le → mono 16 kHz s16le. */
export function stereo48kToMono16k(chunks: Buffer[]): Buffer {
  const samples: number[] = []
  let idx48k = 0
  for (const chunk of chunks) {
    const pairs = Math.floor(chunk.byteLength / 4)
    for (let i = 0; i < pairs; i++) {
      const l = chunk.readInt16LE(i * 4)
      const r = chunk.readInt16LE(i * 4 + 2)
      const mono = Math.round((l + r) / 2)
      if (idx48k % 3 === 0) {
        samples.push(mono)
      }
      idx48k += 1
    }
  }
  const out = Buffer.alloc(samples.length * 2)
  for (let i = 0; i < samples.length; i++) {
    out.writeInt16LE(Math.max(-32768, Math.min(32767, samples[i]!)), i * 2)
  }
  return out
}

export function writeMonoWav16k(path: string, pcm16k: Buffer): void {
  mkdirSync(dirnameOf(path), { recursive: true })
  const sampleRate = 16_000
  const dataBytes = pcm16k.byteLength
  const header = Buffer.alloc(44)
  header.write('RIFF', 0)
  header.writeUInt32LE(36 + dataBytes, 4)
  header.write('WAVE', 8)
  header.write('fmt ', 12)
  header.writeUInt32LE(16, 16)
  header.writeUInt16LE(1, 20)
  header.writeUInt16LE(1, 22)
  header.writeUInt32LE(sampleRate, 24)
  header.writeUInt32LE(sampleRate * 2, 28)
  header.writeUInt16LE(2, 32)
  header.writeUInt16LE(16, 34)
  header.write('data', 36)
  header.writeUInt32LE(dataBytes, 40)
  writeFileSync(path, Buffer.concat([header, pcm16k]))
}

function dirnameOf(filePath: string): string {
  const idx = filePath.lastIndexOf('/')
  return idx >= 0 ? filePath.slice(0, idx) : '.'
}

/** Tap echo agent mixed PCM on the PC outbound track (hop a) — mix pump is the sole writer. */
export function wrapOutboundTrackForPcmCapture<T extends LocalAudioTrack>(
  track: T,
  capture: MultiSessionPcmCapture,
): T {
  const trackAny = track as T & { __pcmOutWrapped?: boolean }
  if (trackAny.__pcmOutWrapped) return track

  const origWrite = trackAny.writeSample.bind(trackAny)
  trackAny.writeSample = async (data: Uint8Array, durationMs: number) => {
    capture.recordOutboundPcm(Buffer.from(data))
    return origWrite(data, durationMs)
  }
  trackAny.__pcmOutWrapped = true
  return track
}

/** Tap speaker inbound readSample (hop b) — mirrors SessionRecorder.wrapInboundTrack. */
export function wrapInboundTrackForPcmCapture(
  track: RemoteAudioTrack,
  capture: MultiSessionPcmCapture,
): RemoteAudioTrack {
  const flagged = track as RemoteAudioTrack & { __pcmInWrapped?: boolean }
  if (flagged.__pcmInWrapped) return track

  const orig = track.readSample.bind(track)
  track.readSample = async () => {
    const pcm = await orig()
    capture.recordInboundRead(pcm, performance.now())
    return pcm
  }
  flagged.__pcmInWrapped = true
  return track
}

export interface BurstHopVerdictInput {
  out: PcmBurstMetrics
  rx: PcmBurstMetrics
  recognized: string
  missingEchoPrefix: boolean
}

const GAP_STRETCHED_MS = 700
const GAP_HEALTHY_MAX_MS = 350

export function localizeBurstFailure(input: BurstHopVerdictInput): PcmFailureVerdict {
  const { out, rx, missingEchoPrefix } = input

  if (out.gapAfterFirstBurstMs >= GAP_STRETCHED_MS) {
    return 'sender: stretched inter-sentence gap under TTS pool contention'
  }

  if (
    out.gapAfterFirstBurstMs <= GAP_HEALTHY_MAX_MS &&
    (rx.gapAfterFirstBurstMs >= GAP_STRETCHED_MS ||
      (out.firstBurstMs > 0 && rx.firstBurstMs < out.firstBurstMs * 0.7))
  ) {
    return 'transport/pacing'
  }

  if (
    missingEchoPrefix &&
    out.gapAfterFirstBurstMs <= GAP_HEALTHY_MAX_MS &&
    rx.gapAfterFirstBurstMs <= GAP_HEALTHY_MAX_MS &&
    rx.firstBurstMs >= out.firstBurstMs * 0.7
  ) {
    return 'receiver STT gating/decode'
  }

  return 'unknown'
}

/** @deprecated use localizeBurstFailure */
export function localizePcmFailure(metrics: PcmHopMetrics): PcmFailureVerdict {
  return localizeBurstFailure({
    out: metrics.outBurst,
    rx: metrics.rxBurst,
    recognized: metrics.recognized,
    missingEchoPrefix: !transcriptHasPrefixBeforeCounting(metrics.recognized),
  })
}

export function formatMergedHopTable(
  rows: Array<{
    sessionId: string
    metrics: PcmHopMetrics & {
      agentSpeakingStartAt: number | null
      outMsRatio?: number | null
    }
  }>,
): string {
  const lines = [
    '=== Per-hop PCM (echo reply window) ===',
    'session | outMs | outMsRatio | rxMs | outVoicedMs | rxVoicedMs | agentSpeakingStartAt | firstRxNonSilentAt | userSpeakingStartAt | maxRxGapMs | firstPartialAt | recognized',
  ]
  for (const row of rows) {
    const m = row.metrics
    const ratio = m.outMsRatio != null ? m.outMsRatio.toFixed(2) : '—'
    const preview = m.recognized.length > 40 ? `${m.recognized.slice(0, 40)}…` : m.recognized
    lines.push(
      `${row.sessionId.padEnd(7)} | ${String(m.outMs).padStart(5)} | ${ratio.padStart(10)} | ${String(m.rxMs).padStart(4)} | ${String(m.outVoicedMs).padStart(11)} | ${String(m.rxVoicedMs).padStart(10)} | ${fmtAt(m.agentSpeakingStartAt)} | ${fmtAt(m.firstRxNonSilentAt)} | ${fmtAt(m.userSpeakingStartAt)} | ${String(m.maxRxGapMs).padStart(10)} | ${fmtAt(m.firstPartialAt)} | ${preview}`,
    )
  }
  return lines.join('\n')
}

export function formatBurstMetricTable(
  rows: Array<{
    sessionId: string
    out: PcmBurstMetrics
    rx: PcmBurstMetrics
    partialMinusAgentStartMs: number | null
    recognized: string
  }>,
): string {
  const lines = [
    '=== Per-hop PCM burst/gap (echo reply window) ===',
    'session | out.firstBurstMs | out.gapAfterFirstBurstMs | rx.firstBurstMs | rx.gapAfterFirstBurstMs | partialAt-agentStart | recognized',
  ]
  for (const row of rows) {
    const preview = row.recognized.length > 40 ? `${row.recognized.slice(0, 40)}…` : row.recognized
    lines.push(
      `${row.sessionId.padEnd(7)} | ${String(row.out.firstBurstMs).padStart(16)} | ${String(row.out.gapAfterFirstBurstMs).padStart(24)} | ${String(row.rx.firstBurstMs).padStart(15)} | ${String(row.rx.gapAfterFirstBurstMs).padStart(23)} | ${fmtAt(row.partialMinusAgentStartMs).padStart(20)} | ${preview}`,
    )
  }
  return lines.join('\n')
}

export function formatPcmFailureVerdicts(
  rows: Array<{
    sessionId: string
    verdict: PcmFailureVerdict
    metrics: BurstHopVerdictInput & { partialMinusAgentStartMs: number | null }
  }>,
): string {
  if (rows.length === 0) return ''
  const lines = ['=== PCM localization (failing sessions) ===']
  for (const row of rows) {
    const { out, rx } = row.metrics
    lines.push(
      `${row.sessionId}: ${row.verdict} — out.gap=${out.gapAfterFirstBurstMs}ms out.firstBurst=${out.firstBurstMs}ms rx.gap=${rx.gapAfterFirstBurstMs}ms rx.firstBurst=${rx.firstBurstMs}ms partial-agentStart=${fmtAt(row.metrics.partialMinusAgentStartMs)}`,
    )
  }
  return lines.join('\n')
}

/** @deprecated */
export function formatPcmHopTable(
  rows: Array<{ sessionId: string; metrics: PcmHopMetrics }>,
): string {
  return formatBurstMetricTable(
    rows.map((row) => ({
      sessionId: row.sessionId,
      out: row.metrics.outBurst,
      rx: row.metrics.rxBurst,
      partialMinusAgentStartMs:
        row.metrics.firstPartialAt != null && row.metrics.agentSpeakingStartAt != null
          ? row.metrics.firstPartialAt - row.metrics.agentSpeakingStartAt
          : null,
      recognized: row.metrics.recognized,
    })),
  )
}

function fmtAt(value: number | null): string {
  return value == null ? '—' : value.toFixed(0)
}

export function transcriptMissingEchoPrefix(recognized: string): boolean {
  return !transcriptHasPrefixBeforeCounting(recognized)
}
