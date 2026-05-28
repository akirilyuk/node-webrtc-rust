/**
 * @packageDocumentation
 * Voice agent API for VAD, STT/TTS vendors, barge-in, and speech events.
 *
 * Import from `@node-webrtc-rust/sdk/voice` to build conversational loops without
 * reimplementing PCM timing or vendor HTTP/WebSocket plumbing.
 */
export { VoiceAgent } from './VoiceAgent'
export type {
  BargeInConfig,
  EventDeliveryMode,
  EventsConfig,
  SpeechEvent,
  SpeechEventListener,
  SpeechEventName,
  SpeechEventType,
  SttConfig,
  SttVendor,
  TtsConfig,
  TtsVendor,
  VadConfig,
  VadSampleRate,
  VoiceAgentConfig,
  VoiceAttachOptions,
} from './types'

export { version } from '@node-webrtc-rust/bindings'
