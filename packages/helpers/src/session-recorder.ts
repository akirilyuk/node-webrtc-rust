/**
 * Session stereo audio recorder — wraps native {@link JsSessionRecorder}.
 *
 * Channel layout (48 kHz s16le stereo export):
 *   L = outbound (client mic / TTS on local outbound track)
 *   R = inbound (agent ready TTS + echo on remote inbound track)
 *
 * Env:
 *   NWR_RECORD_FORMAT       `opus` | `wav` (default `opus`)
 *   NWR_RECORD_BITRATE_BPS  Opus bitrate hint (default 256000; native uses 256 kbps today)
 *   NWR_RECORD_MAX_SEC      capture cap (default 90)
 */

import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'

import { JsSessionAudioFormat, JsSessionRecorder } from '@node-webrtc-rust/bindings'
import type { RemoteAudioTrack } from '@node-webrtc-rust/sdk'

const WRAPPED_OUT = Symbol('sessionRecorderOutbound')
const WRAPPED_IN = Symbol('sessionRecorderInbound')

export const SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS = 256_000
export const SESSION_RECORDER_DEFAULT_MAX_SEC = 90

export type SessionRecorderFormat = 'wav' | 'opus'

export type SessionRecorderOptions = {
  format?: SessionRecorderFormat
  /**
   * Opus export bitrate (bps). Parsed from `NWR_RECORD_BITRATE_BPS`; native finalize
   * currently encodes at 256 kbps — this field is reserved for a future NAPI override.
   */
  bitrateBps?: number
  maxDurationMs?: number
}

export type SessionRecorderFinalizeResult = {
  outputPath: string
  format: SessionRecorderFormat
  durationMs: number
  outboundFrames: number
  inboundFrames: number
}

function parsePositiveInt(raw: string | undefined, fallback: number): number {
  const n = Number.parseInt(raw ?? '', 10)
  return Number.isFinite(n) && n > 0 ? n : fallback
}

function parseFormat(
  raw: string | undefined,
  fallback: SessionRecorderFormat,
): SessionRecorderFormat {
  if (raw == null || raw.trim() === '') return fallback
  const v = raw.trim().toLowerCase()
  if (v === 'wav') return 'wav'
  if (v === 'opus') return 'opus'
  return fallback
}

/** Resolve recorder options from process env (or override env for tests). */
export function resolveSessionRecorderOptionsFromEnv(
  env: NodeJS.ProcessEnv = process.env,
): SessionRecorderOptions {
  return {
    format: parseFormat(env.NWR_RECORD_FORMAT, 'opus'),
    bitrateBps: parsePositiveInt(
      env.NWR_RECORD_BITRATE_BPS,
      SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS,
    ),
    maxDurationMs:
      parsePositiveInt(env.NWR_RECORD_MAX_SEC, SESSION_RECORDER_DEFAULT_MAX_SEC) * 1000,
  }
}

/**
 * Extract PCM from {@link LocalAudioTrack.setWriteSampleTee} callback args.
 *
 * - nwr ≥ Fatal TSFN: `(data, durationMs)`
 * - nwr 0.6.23 CalleeHandled: `(null|Error, data, durationMs)` — first Buffer wins
 */
export function pcmFromWriteSampleTeeArgs(args: readonly unknown[]): Buffer | null {
  for (const arg of args) {
    if (Buffer.isBuffer(arg)) return arg
    if (arg instanceof Uint8Array) return Buffer.from(arg)
  }
  return null
}

/** Vendor-agnostic stereo session recorder (L=outbound, R=inbound @ 48 kHz). */
export class SessionRecorder {
  private readonly inner: JsSessionRecorder
  private readonly format: SessionRecorderFormat
  /** Opus bitrate hint — native export is 256 kbps until NAPI accepts override. */
  readonly bitrateBps: number
  readonly maxDurationMs: number

  constructor(options?: SessionRecorderOptions) {
    const maxDurationMs = options?.maxDurationMs ?? SESSION_RECORDER_DEFAULT_MAX_SEC * 1000
    this.inner = new JsSessionRecorder(maxDurationMs)
    this.format = options?.format ?? 'opus'
    this.bitrateBps = options?.bitrateBps ?? SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS
    this.maxDurationMs = maxDurationMs
  }

  get isClosed(): boolean {
    return this.inner.isClosed
  }

  pushOutbound(pcm: Uint8Array | Buffer): void {
    this.inner.pushOutbound(Buffer.from(pcm))
  }

  pushInbound(pcm: Uint8Array | Buffer): void {
    this.inner.pushInbound(Buffer.from(pcm))
  }

  wrapOutboundTrack<T extends { writeSample: Function }>(track: T): T {
    const flagged = track as T & { [WRAPPED_OUT]?: boolean }
    if (flagged[WRAPPED_OUT]) return track

    const trackAny = track as T & {
      setWriteSampleTee?: (cb: ((...args: unknown[]) => void) | null) => void
      native?: {
        setWriteSampleTee?: (cb: ((...args: unknown[]) => void) | null) => void
      }
      writeSample: (data: Uint8Array, durationMs: number) => Promise<void>
    }
    const setTee =
      typeof trackAny.setWriteSampleTee === 'function'
        ? trackAny.setWriteSampleTee.bind(trackAny)
        : typeof trackAny.native?.setWriteSampleTee === 'function'
          ? trackAny.native.setWriteSampleTee.bind(trackAny.native)
          : null
    if (setTee) {
      setTee((...args: unknown[]) => {
        const pcm = pcmFromWriteSampleTeeArgs(args)
        if (pcm) this.pushOutbound(pcm)
      })
    }

    const orig = trackAny.writeSample.bind(trackAny)
    trackAny.writeSample = async (data: Uint8Array, durationMs: number) => {
      this.pushOutbound(data)
      return orig(data, durationMs)
    }
    flagged[WRAPPED_OUT] = true
    return track
  }

  wrapInboundTrack(track: RemoteAudioTrack): RemoteAudioTrack {
    const flagged = track as RemoteAudioTrack & { [WRAPPED_IN]?: boolean }
    if (flagged[WRAPPED_IN]) return track
    const orig = track.readSample.bind(track)
    track.readSample = async () => {
      const pcm = await orig()
      this.pushInbound(pcm)
      return pcm
    }
    flagged[WRAPPED_IN] = true
    return track
  }

  /** Encode and write session audio to `outputPath`. Closes the native recorder. */
  finalize(outputPath: string): SessionRecorderFinalizeResult {
    const jsFormat = this.format === 'opus' ? JsSessionAudioFormat.Opus : JsSessionAudioFormat.Wav
    const result = this.inner.finalize(jsFormat)
    mkdirSync(dirname(outputPath), { recursive: true })
    writeFileSync(outputPath, result.data)
    const format: SessionRecorderFormat =
      result.format === JsSessionAudioFormat.Opus ? 'opus' : 'wav'
    return {
      outputPath,
      format,
      durationMs: result.durationMs,
      outboundFrames: result.outboundFrames,
      inboundFrames: result.inboundFrames,
    }
  }
}
