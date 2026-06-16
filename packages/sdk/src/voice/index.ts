/**
 * @packageDocumentation
 * Voice agent API for VAD, STT/TTS vendors, barge-in, and speech events.
 *
 * Import from `@node-webrtc-rust/sdk/voice` to build conversational loops without
 * reimplementing PCM timing or vendor HTTP/WebSocket plumbing.
 *
 * **Docs:** [VOICE-API.md](../../VOICE-API.md) (exports + events) ·
 * [VOICE-VAD-AND-BARGE-IN.md](../../VOICE-VAD-AND-BARGE-IN.md) (tuning)
 *
 * @module
 */

/** Main session type — one instance per WebRTC conversation. */
export { VoiceAgent } from './VoiceAgent.js'

/** Stable event name constants for tests and harnesses. */
export { SPEECH_EVENT_TYPE } from './types.js'

/** VAD presets — prefer {@link VOICE_AGENT_VAD_PRESET} for production bots. */
export { DEFAULT_VOICE_AGENT_VAD, VOICE_AGENT_VAD_PRESET } from './defaults.js'

/** `VOICE_DEBUG` helpers for TypeScript-side logging. */
export { isVoiceDebugEnabled, voiceDebugLog } from './debug.js'
/** Browser ↔ Node control channel helpers (`voice-control` label). */
export {
  VOICE_CONTROL_CHANNEL_LABEL,
  VOICE_SYNC_CHANNEL_LABEL,
  forwardVoiceAgentSpeechToDataChannel,
  parseVoiceControlClientMessage,
  speechEventToControlMessage,
  wireVoiceAgentToDataChannel,
  wireVoiceControlSpeakHandler,
} from './speech-event-bridge.js'
export type {
  VoiceControlClientMessage,
  VoiceControlServerMessage,
  VoiceControlSpeakMessage,
  VoiceControlSpeechEventMessage,
  WireVoiceAgentToDataChannelOptions,
} from './speech-event-bridge.js'
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
} from './types.js'

/** Native binding version string. */
export { version } from '@node-webrtc-rust/bindings'
