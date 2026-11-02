import { synthesizeTts as nativeSynthesizeTts } from '@node-webrtc-rust/bindings'

import { toJsTtsConfig } from './VoiceAgent.js'
import type { TtsConfig } from './types.js'

/** Failure classes of {@link synthesizeTts}. */
export type SynthesizeTtsErrorCode =
  | 'invalid_input'
  | 'vendor_rate_limited'
  | 'vendor_key_invalid'
  | 'vendor_timeout'
  | 'vendor_error'

/** Rejection of {@link synthesizeTts}. `message` never contains the api key. */
export class SynthesizeTtsError extends Error {
  readonly code: SynthesizeTtsErrorCode

  constructor(code: SynthesizeTtsErrorCode, message: string) {
    super(message)
    this.name = 'SynthesizeTtsError'
    this.code = code
  }
}

/** Result of {@link synthesizeTts}: mono s16le PCM. */
export interface TtsSynthesis {
  pcm: Buffer
  sampleRate: number
  channels: number
}

const CODES: readonly SynthesizeTtsErrorCode[] = [
  'invalid_input',
  'vendor_rate_limited',
  'vendor_key_invalid',
  'vendor_timeout',
  'vendor_error',
]

/** @internal Splits the native `<code>: <message>` rejection into a typed error. */
export function toSynthesizeTtsError(error: unknown): SynthesizeTtsError {
  const text = error instanceof Error ? error.message : String(error)
  const sep = text.indexOf(': ')
  const head = sep > 0 ? text.slice(0, sep) : ''
  const code = CODES.find((c) => c === head)
  return code
    ? new SynthesizeTtsError(code, text.slice(sep + 2))
    : new SynthesizeTtsError('vendor_error', text)
}

/**
 * Synthesizes `text` once with a vendor and returns mono 24 kHz s16le PCM. Runs the same vendor
 * code as {@link VoiceAgent}; pass the vendor key as `config.apiKey`.
 *
 * `text` is 1 to 1000 characters. `timeoutMs` bounds the whole call (default 15000). Rejects with
 * {@link SynthesizeTtsError}.
 */
export async function synthesizeTts(
  config: TtsConfig,
  text: string,
  timeoutMs?: number,
): Promise<TtsSynthesis> {
  try {
    const out = await nativeSynthesizeTts(toJsTtsConfig(config), text, timeoutMs)
    return { pcm: out.pcm, sampleRate: out.sampleRate, channels: out.channels }
  } catch (error) {
    throw toSynthesizeTtsError(error)
  }
}
