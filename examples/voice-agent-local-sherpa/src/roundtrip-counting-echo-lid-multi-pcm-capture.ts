/**
 * Per-hop PCM metrics for multi echo-LID roundtrip (harness only — no Rust changes).
 *
 * Hops:
 *   (a) echo outbound — PCM written to the echo agent's local TTS track
 *   (b) speaker received — PCM from RemoteAudioTrack.readSample on the speaker leg
 *   (c) speaker STT-fed — user_speaking_start / vad_triggered vs first non-silent RX
 */

import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import { pcmFromWriteSampleTeeArgs } from '@node-webrtc-rust/helpers'

import { stereoPcmDurationMs } from './pcm-relay.js'

export const DEFAULT_VAD_ENERGY_THRESHOLD = 0.15

export function isMultiPcmCaptureEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  const raw = env.SHERPA_MULTI_PCM_CAPTURE
  if (raw === undefined || raw === '') return true
  return raw !== '0' && raw.toLowerCase() !== 'false'
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

export interface PcmHopMetrics {
  outMs: number
  outVoicedMs: number
  rxMs: number
  rxVoicedMs: number
  rxReadCount: number
  maxRxGapMs: number
  firstRxNonSilentAt: number | null
  userSpeakingStartAt: number | null
  vadTriggeredAt: number | null
  firstPartialAt: number | null
  recognized: string
}

export type PcmFailureVerdict =
  | 'sender'
  | 'transport/receive buffer'
  | 'receiver-gating-or-decode'
  | 'unknown'

export class MultiSessionPcmCapture {
  private roundT0 = performance.now()
  private echoOutActive = false
  private rxActive = false
  private lastRxResolvedAt: number | null = null

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

  constructor(private readonly vadThreshold = DEFAULT_VAD_ENERGY_THRESHOLD) {}

  resetRound(): void {
    this.roundT0 = performance.now()
    this.echoOutActive = false
    this.rxActive = false
    this.lastRxResolvedAt = null
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
  }

  private relMs(): number {
    return performance.now() - this.roundT0
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

  recordOutboundPcm(pcm: Buffer): void {
    if (!this.echoOutActive || pcm.length === 0) return
    const durationMs = stereoPcmDurationMs(pcm.length)
    this.outMs += durationMs
    if (isVoicedPcm(pcm, this.vadThreshold)) {
      this.outVoicedMs += durationMs
    }
  }

  recordInboundRead(pcm: Buffer, resolvedAtMs: number): void {
    if (!this.rxActive || pcm.length === 0) return
    const durationMs = stereoPcmDurationMs(pcm.length)
    this.rxMs += durationMs
    this.rxReadCount += 1

    if (this.lastRxResolvedAt != null) {
      const gap = resolvedAtMs - this.lastRxResolvedAt
      if (gap > this.maxRxGapMs) {
        this.maxRxGapMs = gap
      }
    }
    this.lastRxResolvedAt = resolvedAtMs

    if (isVoicedPcm(pcm, this.vadThreshold)) {
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
      userSpeakingStartAt: this.userSpeakingStartAt,
      vadTriggeredAt: this.vadTriggeredAt,
      firstPartialAt: this.firstPartialAt,
      recognized,
    }
  }
}

/** Tap echo agent TTS PCM (hop a) — mirrors SessionRecorder.wrapOutboundTrack. */
export function wrapOutboundTrackForPcmCapture<T extends LocalAudioTrack>(
  track: T,
  capture: MultiSessionPcmCapture,
): T {
  const trackAny = track as T & { __pcmOutWrapped?: boolean }
  if (trackAny.__pcmOutWrapped) return track

  const setTee =
    typeof trackAny.setWriteSampleTee === 'function'
      ? trackAny.setWriteSampleTee.bind(trackAny)
      : typeof trackAny.native?.setWriteSampleTee === 'function'
        ? trackAny.native.setWriteSampleTee.bind(trackAny.native)
        : null

  if (setTee) {
    setTee((...args: unknown[]) => {
      const pcm = pcmFromWriteSampleTeeArgs(args)
      if (pcm) capture.recordOutboundPcm(pcm)
    })
  }

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

export function localizePcmFailure(metrics: PcmHopMetrics): PcmFailureVerdict {
  const expectedReplyMs = 2500
  if (metrics.outMs < expectedReplyMs * 0.5) {
    return 'sender'
  }

  if (metrics.outVoicedMs > 200 && metrics.rxVoicedMs < metrics.outVoicedMs * 0.7) {
    return 'transport/receive buffer'
  }

  if (metrics.maxRxGapMs >= 300) {
    return 'receiver-gating-or-decode'
  }

  const firstSttFed = metrics.userSpeakingStartAt ?? metrics.vadTriggeredAt
  if (
    metrics.firstRxNonSilentAt != null &&
    firstSttFed != null &&
    firstSttFed - metrics.firstRxNonSilentAt > 200
  ) {
    return 'receiver-gating-or-decode'
  }

  if (
    metrics.outVoicedMs > 200 &&
    Math.abs(metrics.rxVoicedMs - metrics.outVoicedMs) <= metrics.outVoicedMs * 0.15 &&
    metrics.firstRxNonSilentAt != null &&
    metrics.firstPartialAt != null &&
    metrics.firstPartialAt - metrics.firstRxNonSilentAt > 700
  ) {
    return 'receiver-gating-or-decode'
  }

  return 'unknown'
}

export function formatPcmHopTable(
  rows: Array<{ sessionId: string; metrics: PcmHopMetrics }>,
): string {
  const lines = [
    '=== Per-hop PCM (echo reply window) ===',
    'session | outMs | rxMs | outVoicedMs | rxVoicedMs | firstRxNonSilentAt | userSpeakingStartAt | maxRxGapMs | firstPartialAt | recognized',
  ]
  for (const row of rows) {
    const m = row.metrics
    const preview = m.recognized.length > 48 ? `${m.recognized.slice(0, 48)}…` : m.recognized
    lines.push(
      `${row.sessionId.padEnd(7)} | ${String(m.outMs).padStart(5)} | ${String(m.rxMs).padStart(4)} | ${String(m.outVoicedMs).padStart(11)} | ${String(m.rxVoicedMs).padStart(10)} | ${fmtAt(m.firstRxNonSilentAt)} | ${fmtAt(m.userSpeakingStartAt)} | ${String(m.maxRxGapMs).padStart(10)} | ${fmtAt(m.firstPartialAt)} | ${preview}`,
    )
  }
  return lines.join('\n')
}

function fmtAt(value: number | null): string {
  return value == null ? '—' : value.toFixed(0)
}

export function formatPcmFailureVerdicts(
  rows: Array<{ sessionId: string; metrics: PcmHopMetrics; verdict: PcmFailureVerdict }>,
): string {
  if (rows.length === 0) return ''
  const lines = ['=== PCM localization (failing sessions) ===']
  for (const row of rows) {
    const m = row.metrics
    lines.push(
      `${row.sessionId}: ${row.verdict} — outMs=${m.outMs} outVoicedMs=${m.outVoicedMs} rxVoicedMs=${m.rxVoicedMs} maxRxGapMs=${m.maxRxGapMs} firstRxNonSilentAt=${fmtAt(m.firstRxNonSilentAt)} userSpeakingStartAt=${fmtAt(m.userSpeakingStartAt)} firstPartialAt=${fmtAt(m.firstPartialAt)}`,
    )
  }
  return lines.join('\n')
}
