/**
 * @packageDocumentation
 * Voice agent API for VAD, STT/TTS vendors, barge-in, and speech events.
 *
 * Import from `@node-webrtc-rust/sdk/voice` to build conversational loops without
 * reimplementing PCM timing or vendor HTTP/WebSocket plumbing.
 */
export { VoiceAgent } from './VoiceAgent'
export { SPEECH_EVENT_TYPE } from './types'
export { DEFAULT_VOICE_AGENT_VAD, VOICE_AGENT_VAD_PRESET } from './defaults'
export { isVoiceDebugEnabled, voiceDebugLog } from './debug'
export {
  VOICE_CONTROL_CHANNEL_LABEL,
  forwardVoiceAgentSpeechToDataChannel,
  parseVoiceControlClientMessage,
  speechEventToControlMessage,
  wireVoiceAgentToDataChannel,
  wireVoiceControlSpeakHandler,
} from './speech-event-bridge'
export type {
  VoiceControlClientMessage,
  VoiceControlServerMessage,
  VoiceControlSpeakMessage,
  VoiceControlSpeechEventMessage,
  WireVoiceAgentToDataChannelOptions,
} from './speech-event-bridge'
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
