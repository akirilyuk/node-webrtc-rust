/**
 * Speech event and configuration types for {@link VoiceAgent}.
 *
 * Runtime behavior (VAD, `gateStt`, barge-in, finalize) is implemented in Rust
 * (`node-webrtc-rust-speech`). See [VOICE-API.md](../../VOICE-API.md) and
 * [VOICE-VAD-AND-BARGE-IN.md](../../VOICE-VAD-AND-BARGE-IN.md).
 *
 * @packageDocumentation
 */

import type { LocalAudioTrack } from '../LocalAudioTrack'
import type { RemoteAudioTrack } from '../RemoteAudioTrack'

/** How {@link SpeechEvent} is delivered: Node callbacks, `speechEvents()` iterator, or both. */
export type EventDeliveryMode = 'callback' | 'stream' | 'both'

/** VAD internal sample rate. Inbound WebRTC PCM is resampled to mono 16 kHz for STT. */
export type VadSampleRate = 8000 | 16000

/**
 * Barge-in: stop agent TTS playback and emit `barge_in`.
 *
 * With `requireSttPartial: true` (default), interrupt during **agent TTS** waits for a
 * qualifying `user_speech_partial` — coughs and tones that do not transcribe do not cut playback.
 */
export interface BargeInConfig {
  /** Master switch for flush + `barge_in` event. Default true. */
  enabled?: boolean
  /**
   * When true (default), inbound VAD `SpeechStart` can trigger barge-in (`vad.enabled` required).
   * When false, only {@link VoiceAgent.flushTts} triggers barge-in — no auto-interrupt on noise.
   */
  useVad?: boolean
  /** Clear pending TTS PCM when barge-in runs. Default true. */
  flushTts?: boolean
  /**
   * While agent TTS is playing, defer barge-in until STT emits a qualifying partial
   * (semantic interrupt). Default true. Requires STT on the agent.
   */
  requireSttPartial?: boolean
  /** Minimum trimmed partial length to trigger barge when `requireSttPartial` is true. Default 2. */
  minSttPartialChars?: number
  /**
   * Optional: ignore VAD barge for this many ms after agent TTS starts (speaker echo).
   * Default 0. Prefer `requireSttPartial` for most setups.
   */
  agentPlaybackGuardMs?: number
}

/**
 * Voice activity detection and STT gating.
 *
 * Prefer {@link VOICE_AGENT_VAD_PRESET} for voice bots; use {@link DEFAULT_VOICE_AGENT_VAD}
 * to match Rust defaults exactly.
 */
export interface VadConfig {
  /** Master VAD switch. Default true. */
  enabled?: boolean
  /**
   * `energy` = RMS VAD (default native build).
   * `silero` = neural VAD if the `.node` was built with `silero-vad`.
   */
  provider?: 'energy' | 'silero'
  /** Energy: ~0.05–0.2. Silero: speech probability ~0.3–0.6. */
  threshold?: number
  /** Minimum voiced time before `user_speaking_start`. Default 250 ms. */
  minSpeechDurationMs?: number
  /**
   * Silence duration before internal `SpeechEnd` (intra-utterance gaps).
   * Default 500 ms in preset. Not inter-phrase batch spacing.
   */
  minSilenceDurationMs?: number
  /** Pre-roll ring capacity (ms); fed to STT at `SpeechStart` when `gateStt`. Default 300. */
  speechPadMs?: number
  sampleRate?: VadSampleRate
  bargeIn?: BargeInConfig
  /**
   * When true, STT only receives PCM while the gate is open (speech, hold, closing).
   * `user_speaking_end` timing follows `sttGateHoldMs` — see VOICE-VAD-AND-BARGE-IN.md.
   */
  gateStt?: boolean
  /** When `gateStt` is true, feed STT during VAD pending speech (WebRTC lead-in). Default true. */
  gateSttOpenOnPending?: boolean
  /** After VAD speech end, keep feeding STT (ms). Default 1000. */
  sttGateHoldMs?: number
  /** After `vad_triggered`, emit `user_stt_not_found` when no partial within this window (ms). Default 4000. */
  sttListenTimeoutMs?: number
  /** Grace after last partial or VAD `SpeechEnd` before forcing `user_speech_final` (ms). Default 1500. */
  utteranceFinalizeTimeoutMs?: number
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
  /**
   * Real-time silence (ms) on outbound audio after each TTS utterance. `0` disables.
   * When unset, derived from VAD gate hold + min silence + 250 ms.
   */
  postUtteranceSilenceMs?: number
}

/** Full configuration for {@link VoiceAgent}. */
export interface VoiceAgentConfig {
  vad?: VadConfig
  events?: EventsConfig
  stt?: SttConfig
  tts?: TtsConfig
  /** Trailing outbound silence after TTS (ms). Deploy JSON may set `tts.postUtteranceSilenceMs`. */
  postUtteranceSilenceMs?: number
}

/**
 * Session-scoped OpenTelemetry attributes and W3C trace propagation for {@link VoiceAgent.start}.
 */
export interface VoiceSessionContext {
  sessionId?: string
  traceId?: string
  projectId?: string
  orgId?: string
  buildId?: string
  /** W3C `traceparent` header value from upstream HTTP/gRPC. */
  traceparent?: string
}

/** Options for {@link VoiceAgent.sendTextToTTS}. */
export interface SendTextToTtsOptions {
  /** When true, resolve as soon as the utterance is queued. Default: wait for synthesis + playback. */
  nonBlocking?: boolean
}

/**
 * Speech lifecycle events from the native pipeline.
 *
 * **Agent events** (`agent_speaking_*`) are emitted only on the VoiceAgent that plays TTS,
 * not on a separate listener peer in a two-agent loopback.
 */
export type SpeechEventType =
  | 'user_speaking_start'
  | 'user_speaking_end'
  | 'user_speech_partial'
  | 'user_speech_final'
  | 'agent_speaking_start'
  | 'agent_speaking_end'
  | 'vad_triggered'
  | 'stt_stream_start'
  | 'stt_stream_end'
  | 'user_stt_start'
  | 'user_stt_end'
  | 'user_stt_not_found'
  | 'barge_in'
  | 'error'

/**
 * Runtime names for {@link SpeechEventType} — use in tests and E2E harnesses
 * instead of string literals.
 */
export const SPEECH_EVENT_TYPE = {
  userSpeakingStart: 'user_speaking_start',
  userSpeakingEnd: 'user_speaking_end',
  userSpeechPartial: 'user_speech_partial',
  userSpeechFinal: 'user_speech_final',
  agentSpeakingStart: 'agent_speaking_start',
  agentSpeakingEnd: 'agent_speaking_end',
  vadTriggered: 'vad_triggered',
  sttStreamStart: 'stt_stream_start',
  sttStreamEnd: 'stt_stream_end',
  userSttStart: 'user_stt_start',
  userSttEnd: 'user_stt_end',
  userSttNotFound: 'user_stt_not_found',
  bargeIn: 'barge_in',
  error: 'error',
} as const satisfies Record<string, SpeechEventType>

/** Payload for callback and `speechEvents()` delivery. */
export interface SpeechEvent {
  type: SpeechEventType
  /** Present on `user_speech_*` and sometimes on errors. */
  text?: string
  /** Present on `error`. */
  error?: string
}

/** Tracks for one peer connection session. */
export interface VoiceAttachOptions {
  peerConnection?: unknown
  /** User → agent audio (`readSample` loop). */
  inboundTrack: RemoteAudioTrack
  /** Agent → user audio (TTS PCM). */
  outboundTrack: LocalAudioTrack
}

export type SpeechEventListener = (event: SpeechEvent) => void

/** Event name for `on()` / `off()` — specific type or `'speech'` for all. */
export type SpeechEventName = SpeechEventType | 'speech'
