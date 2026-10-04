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
  /**
   * Minimum whitespace-separated tokens (with ≥1 alphanumeric) in the STT partial before
   * barge when `requireSttPartial` is true. Default **2** — rejects mid-word fragments
   * (`"St"`) and single-token noise.
   */
  minSttPartialTokens?: number
  /**
   * @deprecated Use {@link minSttPartialTokens}. Legacy alias — same numeric meaning
   * (token count, not character length). Ignored when `minSttPartialTokens` is set.
   */
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

export type SttVendor =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'google'
  | 'assemblyai'
  | 'groq'
  | 'azure'
  | 'aws'
  | 'local-sherpa'
  /** @internal cloud runner only */
  | 'cluster-sherpa'
  | 'mock'

export type TtsVendor =
  | 'openai'
  | 'deepgram'
  | 'elevenlabs'
  | 'google'
  | 'cartesia'
  | 'groq'
  | 'azure'
  | 'aws'
  | 'local-sherpa'
  /** @internal cloud runner only */
  | 'cluster-sherpa'
  | 'mock'

export interface SttConfig {
  provider: SttVendor
  model?: string
  /** Directory with Sherpa ONNX weights (tokens.txt + encoder/decoder/joiner .onnx). */
  modelPath?: string
  language?: string
  apiKey?: string
  /** @internal gRPC speech-service / pool Service URL (runner only). */
  endpoint?: string
}

export interface TtsConfig {
  provider: TtsVendor
  model?: string
  /** Directory with Sherpa VITS/Piper weights (model.onnx, tokens.txt, espeak-ng-data). */
  modelPath?: string
  /** Speaker id for multi-speaker Piper models (default 0). */
  voice?: string
  apiKey?: string
  /** @internal gRPC speech-service / pool Service URL (runner only). */
  endpoint?: string
  /**
   * Real-time silence (ms) on outbound audio after each TTS utterance. `0` disables.
   * When unset, derived from VAD gate hold + min silence + 250 ms.
   */
  postUtteranceSilenceMs?: number
}

/**
 * Offline spoken-language identification (e.g. Sherpa Whisper tiny).
 *
 * Emits `user_language` only — does **not** call {@link VoiceAgent.updateStt} or
 * {@link VoiceAgent.updateTts}. Wire LID to mid-session swap in your application if desired.
 */
export interface LanguageIdConfig {
  /** Default true when `modelPath` is set. Set `false` to disable. */
  enabled?: boolean
  /** Directory with Whisper encoder/decoder ONNX for spoken language ID. */
  modelPath?: string
  /** Optional ISO 639-1 allowlist; other detected codes are ignored. */
  allowlist?: string[]
  /** Minimum buffered speech (ms) before the first identify attempt. Default 1000. */
  minSpeechMs?: number
  /**
   * @deprecated Use `timing: 'continuous'`. When `true`, re-run identify during a long
   * utterance after each pass completes. Default (unset/false): once per utterance. Uses extra CPU and can starve Piper TTS,
   * stretching gaps between sentences so short first TTS bursts may be missed.
   */
  continuous?: boolean
  /**
   * When language ID runs. Default `'end_of_utterance'`: one identify at utterance close.
   * `'early'`: exactly one identify as soon as `minSpeechMs` of speech is buffered, never again
   * for that utterance (same CPU cost as `end_of_utterance`, but earlier). `'continuous'`:
   * repeated passes during a long utterance (more CPU). Takes precedence over `continuous`.
   */
  timing?: LanguageIdTiming
  /** Maximum PCM clip (ms) fed to the identifier per pass. Default 5000. */
  lidMaxClipMs?: number
  /** Fault-path max wait (ms) when gating `user_speech_final` on hung LID. Default 3000. */
  lidGateMaxWaitMs?: number
  /**
   * When true, gate final/TTS on in-flight LID (local Sherpa TTS only by default).
   * When false, final and TTS are never delayed; `userLanguage` follows when identify completes.
   */
  ttsExclusion?: boolean
}

/** Full configuration for {@link VoiceAgent}. */
export interface VoiceAgentConfig {
  vad?: VadConfig
  events?: EventsConfig
  stt?: SttConfig
  tts?: TtsConfig
  languageId?: LanguageIdConfig
  /** `replayLastUtterance()` tuning. */
  replay?: ReplayConfig
  /** Trailing outbound silence after TTS (ms). Deploy JSON may set `tts.postUtteranceSilenceMs`. */
  postUtteranceSilenceMs?: number
  /**
   * Inbound RNNoise before VAD/downsample. Default off (`provider` omitted or `'none'`).
   * Operates on stereo 48 kHz PCM; adds ~10 ms algorithmic delay. Not caller isolation.
   */
  noiseSuppression?: {
    provider?: 'none' | 'rnnoise'
  }
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
  | 'user_language'
  /** No language decision for this utterance (`reason`, `speechMs`); see VOICE-API.md. */
  | 'language_id_skipped'
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
  | 'stt_config_updated'
  | 'tts_config_updated'
  /** {@link VoiceAgent.beginSttHold} accepted (`holdMode`, `bufferedMs`). */
  | 'stt_hold_started'
  /** STT hold finished (`holdOutcome`, `bufferedMs`, `droppedMs`). */
  | 'stt_hold_ended'
  /** Host coordinator: switch started (not emitted by native VoiceAgent alone). */
  | 'voice_language_switching'
  /** Host coordinator: switch succeeded. */
  | 'voice_language_changed'
  /** Host coordinator: switch failed (`error` may be set). */
  | 'voice_language_switch_failed'

/**
 * Runtime names for {@link SpeechEventType} — use in tests and E2E harnesses
 * instead of string literals.
 */
export const SPEECH_EVENT_TYPE = {
  userSpeakingStart: 'user_speaking_start',
  userSpeakingEnd: 'user_speaking_end',
  userSpeechPartial: 'user_speech_partial',
  userSpeechFinal: 'user_speech_final',
  userLanguage: 'user_language',
  languageIdSkipped: 'language_id_skipped',
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
  sttConfigUpdated: 'stt_config_updated',
  ttsConfigUpdated: 'tts_config_updated',
  sttHoldStarted: 'stt_hold_started',
  sttHoldEnded: 'stt_hold_ended',
  voiceLanguageSwitching: 'voice_language_switching',
  voiceLanguageChanged: 'voice_language_changed',
  voiceLanguageSwitchFailed: 'voice_language_switch_failed',
} as const satisfies Record<string, SpeechEventType>

/** Payload for callback and `speechEvents()` delivery. */
export interface SpeechEvent {
  type: SpeechEventType
  /** Present on `user_speech_*`, `user_language`, and sometimes on errors. */
  text?: string
  /** ISO 639-1 code on `user_language`. */
  language?: string
  /** Present on `error`. */
  error?: string
  /** Shared across `user_speaking_start` … `user_speech_final` for one utterance. */
  utteranceId?: string
  /** True when this final was produced by {@link VoiceAgent.replayLastUtterance}. */
  replay?: boolean
  /** Original utterance id when `replay` is true. */
  replacesUtteranceId?: string
  /** Runner may set when releasing a held final after a failed language switch. */
  languageMismatch?: boolean
  /** TTS voice on `tts_config_updated`. */
  voice?: string
  modelPath?: string
  endpoint?: string
  /** `stt_hold_started`: the hold mode. */
  holdMode?: SttHoldMode
  /** `stt_hold_ended`: how the hold ended (`failed` also rejects the call that ended it). */
  holdOutcome?: SttHoldOutcome
  /** `stt_hold_started`: PCM already seeded from the open utterance; `stt_hold_ended`: PCM held at the end (ms). */
  bufferedMs?: number
  /** `stt_hold_ended`: PCM dropped by the buffer bound (oldest first) or by `first_utterance` (ms). */
  droppedMs?: number
  /** `language_id_skipped`: why no language decision will be made for this utterance. */
  reason?: LanguageIdSkipReason
  /** `language_id_skipped`: buffered user speech (ms) at the decision point. */
  speechMs?: number
}

/** `languageId.timing` values. */
export type LanguageIdTiming = 'end_of_utterance' | 'early' | 'continuous'

/** `reason` on `language_id_skipped`. */
export type LanguageIdSkipReason = 'too_short' | 'deferred_tts' | 'no_audio' | 'undetermined'

/** What {@link VoiceAgent.beginSttHold} keeps. */
export type SttHoldMode = 'buffer_replay' | 'first_utterance'

/** `holdOutcome` on `stt_hold_ended`. */
export type SttHoldOutcome = 'released_replay' | 'released_drop' | 'cancelled' | 'failed'

/** Options for {@link VoiceAgent.beginSttHold}. */
export interface BeginSttHoldOptions {
  /**
   * `buffer_replay`: keep all user PCM that would have gone to STT, from the start of the
   * utterance that triggered the hold until release. `first_utterance`: keep only that utterance;
   * later speech is dropped (counted in `droppedMs`).
   */
  mode: SttHoldMode
  /** Buffer bound in ms (oldest audio dropped first). Default 45000, capped at 120000. */
  maxBufferMs?: number
}

/** Options for {@link VoiceAgent.releaseSttHold}. */
export interface ReleaseSttHoldOptions {
  /** Decode the held audio through the current (new) STT. `false` drops the buffer. */
  replay: boolean
}

/** `replay` section of {@link VoiceAgentConfig}. */
export interface ReplayConfig {
  /** Max age (ms) of the last finalized utterance for `replayLastUtterance()`. Default 10000, capped at 120000. */
  maxAgeMs?: number
}

/** Options for {@link VoiceAgent.updateTts}. */
export interface UpdateTtsOptions {
  /** When true, cancel in-flight synthesis and flush playback before applying config. */
  cancelInflight?: boolean
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
