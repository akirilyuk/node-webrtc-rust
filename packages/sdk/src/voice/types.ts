/**
 * Speech event and configuration types for the voice agent API.
 */

import type { LocalAudioTrack } from '../LocalAudioTrack'
import type { RemoteAudioTrack } from '../RemoteAudioTrack'

export type EventDeliveryMode = 'callback' | 'stream' | 'both'

export type VadSampleRate = 8000 | 16000

export interface BargeInConfig {
  enabled?: boolean
  flushTts?: boolean
}

export interface VadConfig {
  enabled?: boolean
  provider?: 'silero'
  threshold?: number
  minSpeechDurationMs?: number
  minSilenceDurationMs?: number
  speechPadMs?: number
  sampleRate?: VadSampleRate
  bargeIn?: BargeInConfig
  gateStt?: boolean
}

export interface EventsConfig {
  mode?: EventDeliveryMode
}

export type SttVendor = 'openai' | 'deepgram' | 'google' | 'assemblyai' | 'mock'

export type TtsVendor = 'openai' | 'elevenlabs' | 'google' | 'cartesia' | 'mock'

export interface SttConfig {
  provider: SttVendor
  model?: string
  language?: string
  apiKey?: string
}

export interface TtsConfig {
  provider: TtsVendor
  model?: string
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
