import { appendFileSync, writeFileSync } from 'fs'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

/** Set `TONE_STREAM_DEBUG=1` to log PCM frame stats on every stream tick. */
export const TONE_STREAM_DEBUG = process.env.TONE_STREAM_DEBUG === '1'

const LOG_PATH = join(dirname(fileURLToPath(import.meta.url)), '../tone-debug.log')

if (TONE_STREAM_DEBUG) {
  writeFileSync(LOG_PATH, `[tone-debug] logging to ${LOG_PATH}\n`)
}

function debugLine(line: string): void {
  console.log(line)
  if (TONE_STREAM_DEBUG) {
    appendFileSync(LOG_PATH, `${line}\n`)
  }
}

export interface PcmFrameStats {
  bytes: number
  samples: number
  peak: number
  rms: number
  firstSamples: number[]
  nonzeroSamples: number
}

/** Summarize interleaved stereo 16-bit LE PCM for console debugging. */
export function summarizePcmFrame(frame: Buffer): PcmFrameStats {
  const samples = Math.floor(frame.byteLength / 2)
  let peak = 0
  let sumSquares = 0
  let nonzeroSamples = 0
  const firstSamples: number[] = []

  for (let i = 0; i < samples; i++) {
    const value = frame.readInt16LE(i * 2)
    if (i < 8) firstSamples.push(value)
    const abs = Math.abs(value)
    if (abs > peak) peak = abs
    if (value !== 0) nonzeroSamples++
    sumSquares += value * value
  }

  const rms = samples > 0 ? Math.round(Math.sqrt(sumSquares / samples)) : 0

  return {
    bytes: frame.byteLength,
    samples,
    peak,
    rms,
    firstSamples,
    nonzeroSamples,
  }
}

export function logToneTick(
  peerId: string,
  phase: 'kick' | 'stream' | 'prime',
  tick: number,
  stats: PcmFrameStats,
  meta: {
    writeMs: number
    connectionState: string
    ok: boolean
    error?: string
    durationMs?: number
  },
): void {
  if (!TONE_STREAM_DEBUG) return

  const label =
    phase === 'kick' ? 'KICK (full frame)' : phase === 'prime' ? 'PRIME (960 B)' : `TICK #${tick}`
  const status = meta.ok ? 'ok' : `ERR ${meta.error ?? 'unknown'}`
  const dur = meta.durationMs !== undefined ? ` durMs=${meta.durationMs}` : ''
  debugLine(
    `[tone ${peerId}] ${label} bytes=${stats.bytes} peak=${stats.peak} rms=${stats.rms} ` +
      `nonzero=${stats.nonzeroSamples}/${stats.samples} first=[${stats.firstSamples.join(', ')}] ` +
      `writeMs=${meta.writeMs.toFixed(1)}${dur} pc=${meta.connectionState} ${status}`,
  )
}

/** Log a debug line when TONE_STREAM_DEBUG=1 (also appended to tone-debug.log). */
export function logToneDebug(line: string): void {
  if (!TONE_STREAM_DEBUG) return
  debugLine(line)
}

export { LOG_PATH as TONE_DEBUG_LOG_PATH }
