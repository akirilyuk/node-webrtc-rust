/**
 * Speech event and configuration types for the voice agent API.
 */

import type { LocalAudioTrack } from '../LocalAudioTrack'
import type { RemoteAudioTrack } from '../RemoteAudioTrack'

export type EventDeliveryMode = 'callback' | 'stream' | 'both'

export type VadSampleRate = 8000 | 16000

export interface BargeInConfig {
  /** Master switch for flush + `barge_in` event. Default true. */
  enabled?: boolean
  /**
   * When true (default), inbound VAD `SpeechStart` triggers barge-in (`vad.enabled` required).
   * When false, only `flushTts()` triggers barge-in — avoids auto-interrupt on noise/tones.
   */
  useVad?: boolean
  /** Clear pending TTS PCM when barge-in runs. Default true. */
  flushTts?: boolean
}

/** See VOICE-VAD-AND-BARGE-IN.md — prefer VOICE_AGENT_VAD_PRESET for voice bots. */
export interface VadConfig {
  enabled?: boolean
  /** `energy` = RMS VAD (shipped in default `.node`). `silero` = neural VAD if native built with `silero-vad`. */
  provider?: 'energy' | 'silero'
  /** Energy: ~0.05–0.2. Silero: speech probability ~0.3–0.6. */
  threshold?: number
  minSpeechDurationMs?: number
  minSilenceDurationMs?: number
  /** Pre-roll ring capacity (ms), added to minSpeechDurationMs. Default 300. */
  speechPadMs?: number
  sampleRate?: VadSampleRate
  /** Flush TTS on inbound VAD SpeechStart; requires `enabled: true`. */
  bargeIn?: BargeInConfig
  gateStt?: boolean
  /** When gateStt is true, feed STT during VAD pending speech. Default true. */
  gateSttOpenOnPending?: boolean
  /** Keep feeding STT after VAD speech end (ms). Default 2500. */
  sttGateHoldMs?: number
}

export interface EventsConfig {
  mode?: EventDeliveryMode
}

export type SttVendor = 'openai' | 'deepgram' | 'google' | 'assemblyai' | 'local-sherpa' | 'mock'

export type TtsVendor = 'openai' | 'elevenlabs' | 'google' | 'cartesia' | 'local-sherpa' | 'mock'

export interface SttConfig {
  provider: SttVendor
  model?: string
  /** Directory with Sherpa ONNX weights (tokens.txt + encoder/decoder/joiner .onnx). */
  modelPath?: string
  language?: string
  apiKey?: string
}

export interface TtsConfig {
  provider: TtsVendor
  model?: string
  /** Directory with Sherpa VITS/Piper weights (model.onnx, tokens.txt, espeak-ng-data). */
  modelPath?: string
  /** Speaker id for multi-speaker Piper models (default 0). */
  voice?: string
  apiKey?: string
}

export interface VoiceAgentConfig {
  vad?: VadConfig
  events?: EventsConfig
  stt?: SttConfig
  tts?: TtsConfig
}

export type SpeechEventType =
  | 'user_speaking_start'
  | 'user_speaking_end'
  | 'user_speech_partial'
  | 'user_speech_final'
  | 'agent_speaking_start'
  | 'agent_speaking_end'
  | 'barge_in'
  | 'error'

export interface SpeechEvent {
  type: SpeechEventType
  text?: string
  error?: string
}

export interface VoiceAttachOptions {
  peerConnection?: unknown
  inboundTrack: RemoteAudioTrack
  outboundTrack: LocalAudioTrack
}

export type SpeechEventListener = (event: SpeechEvent) => void

export type SpeechEventName = SpeechEventType | 'speech'
