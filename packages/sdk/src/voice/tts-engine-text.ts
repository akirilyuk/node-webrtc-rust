import { ttsEngineText as nativeTtsEngineText } from '@node-webrtc-rust/bindings'

/** Result of {@link ttsEngineText}. */
export interface TtsEngineText {
  /** The text the Sherpa TTS engine receives. */
  text: string
  /** Letters uppercased relative to the input (0 when nothing changed or `mode` is `off`). */
  adjusted: number
}

/**
 * The exact text the Sherpa TTS engine receives for `text` (sentence-start capitalization).
 * For traces; does not synthesize.
 *
 * Whitespace is collapsed first, as the Sherpa adapters do. `mode` is the agent's
 * `tts.sentenceCase`: `auto` (default) capitalizes sentence starts, `off` returns the text as is.
 */
export function ttsEngineText(text: string, mode: 'auto' | 'off' = 'auto'): TtsEngineText {
  const { text: engineText, adjusted } = nativeTtsEngineText(text, mode)
  return { text: engineText, adjusted }
}
