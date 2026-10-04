import {
  JsEventDeliveryMode,
  JsNoiseSuppressionProvider,
  JsSpeechEventType,
  JsSttHoldMode,
  JsSttVendor,
  JsTtsVendor,
  JsVadSampleRate,
  JsVoiceAgent as NativeVoiceAgent,
  type JsSpeechEvent,
  type JsSttConfig,
  type JsTtsConfig,
  type JsUpdateTtsOptions,
  type JsVadConfig,
  type JsVoiceAgent,
  type JsVoiceAgentConfig,
  type JsVoiceSessionContext,
} from '@node-webrtc-rust/bindings'

import type { RemoteAudioTrack } from '../RemoteAudioTrack'
import { debugEvent, debugFn } from '../debug'
import { isVoiceDebugEnabled, voiceDebugLog } from './debug'
import type {
  BeginSttHoldOptions,
  ReleaseSttHoldOptions,
  SttHoldMode,
  LanguageIdSkipReason,
  SttHoldOutcome,
  EventDeliveryMode,
  SpeechEvent,
  SpeechEventListener,
  SpeechEventName,
  SpeechEventType,
  SttConfig,
  SttVendor,
  TtsConfig,
  TtsVendor,
  UpdateTtsOptions,
  VadConfig,
  VoiceAgentConfig,
  VoiceAttachOptions,
  VoiceSessionContext,
  SendTextToTtsOptions,
} from './types'

const MODULE = 'voice::VoiceAgent'

/** Stereo 48 kHz 20 ms frame (3840 bytes) — matches native TTS drain. */
const INBOUND_SILENCE_FRAME_BYTES = 3840
const INBOUND_FRAME_MS = 20

function isInboundStreamEndError(error: unknown): boolean {
  const message = String(error)
  return (
    message.includes('DataChannel is not opened') ||
    message.includes('ErrClosedPipe') ||
    message.includes('closed pipe')
  )
}

function toJsVadConfig(vad?: VadConfig): JsVadConfig | undefined {
  if (!vad) return undefined
  return {
    enabled: vad.enabled,
    provider: vad.provider,
    threshold: vad.threshold,
    minSpeechDurationMs: vad.minSpeechDurationMs,
    minSilenceDurationMs: vad.minSilenceDurationMs,
    speechPadMs: vad.speechPadMs,
    sampleRate:
      vad.sampleRate === 8000
        ? JsVadSampleRate.Hz8000
        : vad.sampleRate === 16000
          ? JsVadSampleRate.Hz16000
          : undefined,
    bargeIn: vad.bargeIn
      ? {
          enabled: vad.bargeIn.enabled,
          useVad: vad.bargeIn.useVad,
          flushTts: vad.bargeIn.flushTts,
          requireSttPartial: vad.bargeIn.requireSttPartial,
          minSttPartialTokens: vad.bargeIn.minSttPartialTokens,
          minSttPartialChars: vad.bargeIn.minSttPartialChars,
          agentPlaybackGuardMs: vad.bargeIn.agentPlaybackGuardMs,
        }
      : undefined,
    gateStt: vad.gateStt,
    gateSttOpenOnPending: vad.gateSttOpenOnPending,
    sttGateHoldMs: vad.sttGateHoldMs,
    sttListenTimeoutMs: vad.sttListenTimeoutMs,
    utteranceFinalizeTimeoutMs: vad.utteranceFinalizeTimeoutMs,
  }
}

function toJsSttConfig(stt: SttConfig): JsSttConfig {
  return {
    provider: sttVendorToJs(stt.provider),
    model: stt.model,
    modelPath: stt.modelPath,
    language: stt.language,
    apiKey: stt.apiKey,
    endpoint: stt.endpoint,
  }
}

function toJsTtsConfig(tts: TtsConfig): JsTtsConfig {
  return {
    provider: ttsVendorToJs(tts.provider),
    model: tts.model,
    modelPath: tts.modelPath,
    voice: tts.voice,
    apiKey: tts.apiKey,
    endpoint: tts.endpoint,
    postUtteranceSilenceMs: tts.postUtteranceSilenceMs,
  }
}

/** @internal Maps SDK STT vendor strings onto NAPI enums. Unknown runtime values become Mock. */
export function sttVendorToJs(vendor: SttConfig['provider']): JsSttVendor {
  switch (vendor) {
    case 'openai':
      return JsSttVendor.Openai
    case 'deepgram':
      return JsSttVendor.Deepgram
    case 'elevenlabs':
      return JsSttVendor.Elevenlabs
    case 'google':
      return JsSttVendor.Google
    case 'assemblyai':
      return JsSttVendor.Assemblyai
    case 'groq':
      return JsSttVendor.Groq
    case 'azure':
      return JsSttVendor.Azure
    case 'aws':
      return JsSttVendor.Aws
    case 'local-sherpa':
      return JsSttVendor.LocalSherpa
    case 'cluster-sherpa':
      return JsSttVendor.ClusterSherpa
    default:
      return JsSttVendor.Mock
  }
}

/** @internal Maps SDK TTS vendor strings onto NAPI enums. Unknown runtime values become Mock. */
export function ttsVendorToJs(vendor: TtsConfig['provider']): JsTtsVendor {
  switch (vendor) {
    case 'openai':
      return JsTtsVendor.Openai
    case 'deepgram':
      return JsTtsVendor.Deepgram
    case 'elevenlabs':
      return JsTtsVendor.Elevenlabs
    case 'google':
      return JsTtsVendor.Google
    case 'cartesia':
      return JsTtsVendor.Cartesia
    case 'groq':
      return JsTtsVendor.Groq
    case 'azure':
      return JsTtsVendor.Azure
    case 'aws':
      return JsTtsVendor.Aws
    case 'local-sherpa':
      return JsTtsVendor.LocalSherpa
    case 'cluster-sherpa':
      return JsTtsVendor.ClusterSherpa
    default:
      return JsTtsVendor.Mock
  }
}

/** @internal Every `SttVendor` string. Typecheck fails if the union grows without this list. */
export const STT_VENDOR_VALUES = [
  'openai',
  'deepgram',
  'elevenlabs',
  'google',
  'assemblyai',
  'groq',
  'azure',
  'aws',
  'local-sherpa',
  'cluster-sherpa',
  'mock',
] as const satisfies readonly SttVendor[]

/** @internal Every `TtsVendor` string. Typecheck fails if the union grows without this list. */
export const TTS_VENDOR_VALUES = [
  'openai',
  'deepgram',
  'elevenlabs',
  'google',
  'cartesia',
  'groq',
  'azure',
  'aws',
  'local-sherpa',
  'cluster-sherpa',
  'mock',
] as const satisfies readonly TtsVendor[]

type _AssertNever<T extends never> = T
type _SttVendorsCovered = _AssertNever<Exclude<SttVendor, (typeof STT_VENDOR_VALUES)[number]>>
type _TtsVendorsCovered = _AssertNever<Exclude<TtsVendor, (typeof TTS_VENDOR_VALUES)[number]>>
export type InternalVendorCoverage = _SttVendorsCovered | _TtsVendorsCovered

function toJsSessionContext(ctx?: VoiceSessionContext): JsVoiceSessionContext | undefined {
  if (!ctx) return undefined
  return {
    sessionId: ctx.sessionId,
    traceId: ctx.traceId,
    projectId: ctx.projectId,
    orgId: ctx.orgId,
    buildId: ctx.buildId,
    traceparent: ctx.traceparent,
  }
}

function toJsConfig(config?: VoiceAgentConfig): JsVoiceAgentConfig | undefined {
  if (!config) return undefined
  const postUtteranceSilenceMs = config.postUtteranceSilenceMs ?? config.tts?.postUtteranceSilenceMs
  return {
    vad: toJsVadConfig(config.vad),
    events: config.events?.mode ? { mode: eventModeToJs(config.events.mode) } : undefined,
    stt: config.stt ? toJsSttConfig(config.stt) : undefined,
    tts: config.tts ? toJsTtsConfig(config.tts) : undefined,
    languageId: config.languageId
      ? {
          enabled: config.languageId.enabled,
          modelPath: config.languageId.modelPath,
          allowlist: config.languageId.allowlist,
          minSpeechMs: config.languageId.minSpeechMs,
          continuous: config.languageId.continuous,
        }
      : undefined,
    postUtteranceSilenceMs,
    replay: config.replay ? { maxAgeMs: config.replay.maxAgeMs } : undefined,
    noiseSuppression: config.noiseSuppression
      ? {
          provider:
            config.noiseSuppression.provider === 'rnnoise'
              ? JsNoiseSuppressionProvider.Rnnoise
              : config.noiseSuppression.provider === 'none'
                ? JsNoiseSuppressionProvider.None
                : undefined,
        }
      : undefined,
  }
}

function eventModeToJs(mode: EventDeliveryMode): JsEventDeliveryMode {
  switch (mode) {
    case 'callback':
      return JsEventDeliveryMode.Callback
    case 'stream':
      return JsEventDeliveryMode.Stream
    default:
      return JsEventDeliveryMode.Both
  }
}

function fromJsSpeechEvent(event: JsSpeechEvent): SpeechEvent {
  const rawType =
    event.eventType ?? (event as JsSpeechEvent & { event_type?: JsSpeechEventType }).event_type
  const extended = event as JsSpeechEvent & {
    utterance_id?: string
    replaces_utterance_id?: string
    language_mismatch?: boolean
    model_path?: string
    hold_mode?: string
    hold_outcome?: string
    buffered_ms?: number
    dropped_ms?: number
    speech_ms?: number
  }
  return {
    type: jsEventTypeToString(rawType ?? JsSpeechEventType.Error),
    text: event.text ?? undefined,
    language: event.language ?? undefined,
    error: event.error ?? undefined,
    utteranceId: event.utteranceId ?? extended.utterance_id ?? undefined,
    replay: event.replay ?? undefined,
    replacesUtteranceId: event.replacesUtteranceId ?? extended.replaces_utterance_id ?? undefined,
    languageMismatch: event.languageMismatch ?? extended.language_mismatch ?? undefined,
    voice: event.voice ?? undefined,
    modelPath: event.modelPath ?? extended.model_path ?? undefined,
    endpoint: event.endpoint ?? undefined,
    holdMode: (event.holdMode ?? extended.hold_mode ?? undefined) as SttHoldMode | undefined,
    holdOutcome: (event.holdOutcome ?? extended.hold_outcome ?? undefined) as
      | SttHoldOutcome
      | undefined,
    bufferedMs: event.bufferedMs ?? extended.buffered_ms ?? undefined,
    droppedMs: event.droppedMs ?? extended.dropped_ms ?? undefined,
    reason: (event.reason ?? undefined) as LanguageIdSkipReason | undefined,
    speechMs: event.speechMs ?? extended.speech_ms ?? undefined,
  }
}

function jsEventTypeToString(eventType: JsSpeechEventType): SpeechEventType {
  switch (eventType) {
    case JsSpeechEventType.UserSpeakingStart:
      return 'user_speaking_start'
    case JsSpeechEventType.UserSpeakingEnd:
      return 'user_speaking_end'
    case JsSpeechEventType.UserSpeechPartial:
      return 'user_speech_partial'
    case JsSpeechEventType.UserSpeechFinal:
      return 'user_speech_final'
    case JsSpeechEventType.UserLanguage:
      return 'user_language'
    case JsSpeechEventType.LanguageIdSkipped:
      return 'language_id_skipped'
    case JsSpeechEventType.AgentSpeakingStart:
      return 'agent_speaking_start'
    case JsSpeechEventType.AgentSpeakingEnd:
      return 'agent_speaking_end'
    case JsSpeechEventType.VadTriggered:
      return 'vad_triggered'
    case JsSpeechEventType.SttStreamStart:
      return 'stt_stream_start'
    case JsSpeechEventType.SttStreamEnd:
      return 'stt_stream_end'
    case JsSpeechEventType.UserSttStart:
      return 'user_stt_start'
    case JsSpeechEventType.UserSttEnd:
      return 'user_stt_end'
    case JsSpeechEventType.UserSttNotFound:
      return 'user_stt_not_found'
    case JsSpeechEventType.BargeIn:
      return 'barge_in'
    case JsSpeechEventType.SttConfigUpdated:
      return 'stt_config_updated'
    case JsSpeechEventType.TtsConfigUpdated:
      return 'tts_config_updated'
    case JsSpeechEventType.SttHoldStarted:
      return 'stt_hold_started'
    case JsSpeechEventType.SttHoldEnded:
      return 'stt_hold_ended'
    default:
      return 'error'
  }
}

/**
 * Voice agent orchestrating VAD, STT, TTS, and barge-in for one WebRTC session.
 *
 * After {@link attach} and {@link start}, a background loop reads
 * `inboundTrack.readSample()` (20 ms frames) and forwards PCM to the native
 * `processInboundPcm` pipeline. TTS from {@link sendTextToTTS} is drained to
 * `outboundTrack` at real-time cadence.
 *
 * @see [VOICE-API.md](../../VOICE-API.md)
 */
export class VoiceAgent {
  private readonly native: JsVoiceAgent
  private readonly eventsMode: EventDeliveryMode
  private inboundTrack?: RemoteAudioTrack
  private inboundLoop?: Promise<void>
  private speechPullLoop?: Promise<void>
  /** Buffered events for {@link speechEvents} when mode is `'both'` (single pull consumer). */
  private readonly speechStreamQueue: SpeechEvent[] = []
  private readonly speechStreamWaiters: Array<(event: SpeechEvent) => void> = []
  private running = false
  private readonly listeners = new Map<SpeechEventName, Set<SpeechEventListener>>()

  /**
   * @param config — optional VAD/STT/TTS/events; omitted fields use Rust defaults.
   */
  constructor(config?: VoiceAgentConfig) {
    debugFn(MODULE, 'constructor')
    this.eventsMode = config?.events?.mode ?? 'both'
    this.native = new NativeVoiceAgent(toJsConfig(config))
  }

  /**
   * Binds inbound (user) and outbound (agent TTS) tracks for one peer connection.
   * Call before {@link start}.
   */
  async attach(options: VoiceAttachOptions): Promise<void> {
    debugFn(MODULE, 'attach')
    this.inboundTrack = options.inboundTrack
    await this.native.attach(options.outboundTrack.native)
  }

  /** Starts STT, TTS drain, and the inbound PCM loop. Idempotent error if already running. */
  async start(sessionContext?: VoiceSessionContext): Promise<void> {
    debugFn(MODULE, 'start')
    await this.native.start(toJsSessionContext(sessionContext))
    this.running = true
    voiceDebugLog(MODULE, 'native start() complete — starting inbound PCM loop')
    this.startInboundLoop()
    if (this.eventsMode === 'callback' || this.eventsMode === 'both') {
      this.startSpeechEventPullLoop()
    }
  }

  /** Stops STT and the inbound loop. */
  async stop(): Promise<void> {
    debugFn(MODULE, 'stop')
    this.running = false
    await this.native.stop()
  }

  /**
   * Synthesizes `text` via the configured TTS vendor and enqueues PCM for outbound playback.
   * Emits `agent_speaking_start` / `agent_speaking_end` around the drain window.
   *
   * By default waits until synthesis and playback for this utterance finish. Pass
   * `{ nonBlocking: true }` to return once the job is queued.
   */
  async sendTextToTTS(text: string, options?: SendTextToTtsOptions): Promise<void> {
    debugFn(MODULE, 'sendTextToTTS', `chars=${text.length}`)
    await this.native.sendTextToTts(text, options?.nonBlocking ?? undefined)
  }

  /**
   * Clears pending TTS PCM (manual interrupt).
   * Also used internally when barge-in runs with `bargeIn.flushTts: true`.
   */
  async flushTts(): Promise<void> {
    debugFn(MODULE, 'flushTts')
    await this.native.flushTts()
  }

  /**
   * Blocks until outbound TTS queue is drained and `agent_speaking` is false.
   * Can block the Node event loop for long phrases — prefer `agent_speaking_end` events in app code.
   */
  async waitTtsPlaybackIdle(): Promise<void> {
    debugFn(MODULE, 'waitTtsPlaybackIdle')
    await this.native.waitTtsPlaybackIdle()
  }

  /**
   * When `false`, inbound PCM still runs VAD but skips STT and user speech events.
   * TTS playback is unchanged.
   */
  async setSttEnabled(enabled: boolean): Promise<void> {
    debugFn(MODULE, 'setSttEnabled', String(enabled))
    await this.native.setSttEnabled(enabled)
  }

  /** Returns whether STT is enabled for inbound PCM. */
  async sttEnabled(): Promise<boolean> {
    return this.native.sttEnabled()
  }

  /**
   * Queue a new STT config for mid-session language or model change without re-attaching tracks.
   *
   * Native applies the config after the current utterance finalizes (gate hold and STT close
   * included), or immediately when no utterance is in progress. Emits `stt_config_updated`
   * with `language`, `modelPath`, and `endpoint` from the applied config.
   *
   * Does not run automatically on `user_language` — your application decides when to call this.
   *
   * @see {@link replayLastUtterance} to re-decode the utterance that triggered a switch
   * @see [VOICE-API.md](../../VOICE-API.md#mid-session-stttts-language-and-model-switch)
   */
  async updateStt(config: SttConfig): Promise<void> {
    await this.native.updateStt(toJsSttConfig(config))
  }

  /**
   * Queue a new TTS config; native applies it before the next {@link sendTextToTTS} job.
   *
   * When `cancelInflight` is true, cancels in-flight synthesis and flushes playback before
   * swapping vendors (same flush path as barge-in). Emits `tts_config_updated` with `voice`,
   * `modelPath`, and `endpoint`.
   *
   * @see [VOICE-API.md](../../VOICE-API.md#mid-session-stttts-language-and-model-switch)
   */
  async updateTts(config: TtsConfig, options?: UpdateTtsOptions): Promise<void> {
    const jsOptions: JsUpdateTtsOptions | undefined = options
      ? { cancelInflight: options.cancelInflight }
      : undefined
    await this.native.updateTts(toJsTtsConfig(config), jsOptions)
  }

  /**
   * Start a host-controlled STT hold for a switch that takes seconds (cold STT pool).
   *
   * From the moment this resolves, inbound user audio is no longer sent to the current STT and
   * the current STT is no longer polled, so **no `user_speech_partial` / `user_speech_final` from
   * the old model is emitted afterwards**. VAD and `user_speaking_*` events keep flowing, and
   * semantic (STT-partial) barge-in is inactive while held (VAD barge-in still works).
   *
   * - `buffer_replay`: all PCM that would have gone to STT is buffered, starting with the audio of
   *   the utterance in progress (seeded from the 15 s utterance replay ring) until release,
   *   bounded by `maxBufferMs` (default 45000, cap 120000; oldest audio dropped first and counted).
   * - `first_utterance`: only the triggering utterance is kept; later speech is dropped.
   *
   * Emits `stt_hold_started` (`holdMode`, `bufferedMs`). Rejects when a hold is already active.
   * Pair with {@link releaseSttHold} (after {@link updateStt}) or {@link cancelSttHold}.
   *
   * @see [VOICE-API.md](../../VOICE-API.md#stt-hold-slow-stt-swap)
   */
  async beginSttHold(options: BeginSttHoldOptions): Promise<void> {
    await this.native.beginSttHold({
      mode:
        options.mode === 'first_utterance'
          ? JsSttHoldMode.FirstUtterance
          : JsSttHoldMode.BufferReplay,
      maxBufferMs: options.maxBufferMs,
    })
  }

  /**
   * End the hold after the host swapped the STT with {@link updateStt}.
   *
   * `replay: true` decodes the held audio through the new STT and emits `user_speech_final`
   * with `replay: true` (and `replacesUtteranceId` when an original utterance existed), then
   * resumes live audio. Ordering: held audio first, then live audio, no gap and no duplicate
   * (live audio keeps being buffered until the held buffer is empty). `replay: false` drops the
   * buffer. Emits `stt_hold_ended` (`holdOutcome`, `bufferedMs`, `droppedMs`; no transcript text).
   */
  async releaseSttHold(options: ReleaseSttHoldOptions): Promise<void> {
    await this.native.releaseSttHold({ replay: options.replay })
  }

  /**
   * Abort the hold (switch failed): the old STT resumes and receives the buffered audio in order;
   * a still-queued {@link updateStt} config is discarded. Emits `stt_hold_ended` with
   * `holdOutcome: 'cancelled'`.
   */
  async cancelSttHold(): Promise<void> {
    await this.native.cancelSttHold()
  }

  /**
   * Re-feed the last finalized utterance's post-RNNoise PCM into the current STT after
   * {@link updateStt}, producing a new `user_speech_final` with `replay: true` and
   * `replacesUtteranceId` set to the original turn.
   *
   * Call when idle (after the original final). Rejects when no PCM snapshot exists, an
   * utterance is still open, an STT hold is active, the buffer overflowed, or the snapshot is
   * older than `config.replay.maxAgeMs` (default 10 s, cap 120 s).
   *
   * Headless mock check: `npm run start:replay-last-utterance` in
   * `example-voice-agent-local-sherpa-multi-client`.
   *
   * @see [VOICE-API.md](../../VOICE-API.md#replaylastutterance)
   */
  async replayLastUtterance(): Promise<void> {
    await this.native.replayLastUtterance()
  }

  /** Subscribe to `event` or `'speech'` for all event types. */
  on(event: SpeechEventName, listener: SpeechEventListener): this {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, new Set())
    }
    this.listeners.get(event)!.add(listener)
    return this
  }

  off(event: SpeechEventName, listener: SpeechEventListener): this {
    this.listeners.get(event)?.delete(listener)
    return this
  }

  /**
   * Async iterator over speech events (`events.mode` `'stream'` or `'both'`).
   *
   * Active only while {@link start} has run and before {@link stop}.
   * **`agent_speaking_*` events are emitted only on the agent that plays TTS** — not on a
   * separate listener `VoiceAgent` in a two-peer setup.
   */
  async *speechEvents(): AsyncGenerator<SpeechEvent, void, undefined> {
    if (this.eventsMode === 'callback') {
      return
    }
    if (this.eventsMode === 'both') {
      while (this.running) {
        const event = await this.waitSpeechStreamEvent()
        if (event) {
          yield event
        } else {
          await new Promise((resolve) => setTimeout(resolve, 10))
        }
      }
      return
    }
    while (this.running) {
      const event = await this.native.pullSpeechEvent()
      if (event) {
        const speechEvent = fromJsSpeechEvent(event)
        yield speechEvent
      } else {
        await new Promise((resolve) => setTimeout(resolve, 10))
      }
    }
  }

  /** @internal Exposes native handle for tests. */
  getNativeAgent(): JsVoiceAgent {
    return this.native
  }

  private dispatch(event: SpeechEvent): void {
    debugEvent(MODULE, event.type, event.text ?? event.error ?? '')
    this.listeners.get('speech')?.forEach((fn) => fn(event))
    this.listeners.get(event.type)?.forEach((fn) => fn(event))
  }

  /**
   * Native ThreadsafeFunction callbacks are unreliable under Node; pull from the
   * broadcast channel and dispatch to `on()` listeners (and queue for `both` mode).
   */
  private startSpeechEventPullLoop(): void {
    this.speechPullLoop = (async () => {
      while (this.running) {
        try {
          const raw = await this.native.pullSpeechEvent()
          if (!raw) {
            await new Promise((resolve) => setTimeout(resolve, 10))
            continue
          }
          const event = fromJsSpeechEvent(raw)
          this.dispatch(event)
          if (this.eventsMode === 'both') {
            this.enqueueSpeechStreamEvent(event)
          }
        } catch (error: unknown) {
          if (isVoiceDebugEnabled()) {
            voiceDebugLog(MODULE, `speech pull loop error: ${String(error)}`)
          }
          await new Promise((resolve) => setTimeout(resolve, 10))
        }
      }
    })()
  }

  private enqueueSpeechStreamEvent(event: SpeechEvent): void {
    const waiter = this.speechStreamWaiters.shift()
    if (waiter) {
      waiter(event)
    } else {
      this.speechStreamQueue.push(event)
    }
  }

  private waitSpeechStreamEvent(): Promise<SpeechEvent | null> {
    const queued = this.speechStreamQueue.shift()
    if (queued) {
      return Promise.resolve(queued)
    }
    if (!this.running) {
      return Promise.resolve(null)
    }
    return new Promise((resolve) => {
      this.speechStreamWaiters.push(resolve)
    })
  }

  private async injectInboundSilenceTail(totalMs = 1500): Promise<void> {
    const silent = Buffer.alloc(INBOUND_SILENCE_FRAME_BYTES)
    const frameCount = Math.ceil(totalMs / INBOUND_FRAME_MS)
    for (let i = 0; i < frameCount && this.running; i++) {
      await this.native.processInboundPcm(silent, INBOUND_FRAME_MS)
      await new Promise((resolve) => setTimeout(resolve, INBOUND_FRAME_MS))
    }
  }

  private startInboundLoop(): void {
    const track = this.inboundTrack
    if (!track) return

    this.inboundLoop = (async () => {
      let frameCount = 0
      /** True after stream-end until the next successful readSample (multi-turn sessions). */
      let awaitingNextRtpBurst = false
      while (this.running) {
        try {
          const pcm = await track.readSample()
          awaitingNextRtpBurst = false
          frameCount += 1
          if (isVoiceDebugEnabled() && (frameCount === 1 || frameCount % 50 === 0)) {
            voiceDebugLog(MODULE, `inbound pcm frame=${frameCount} bytes=${pcm.length}`)
          }
          await this.native.processInboundPcm(pcm, 20)
        } catch (error: unknown) {
          if (isInboundStreamEndError(error)) {
            voiceDebugLog(MODULE, 'inbound RTP stream ended (receiver stopped)')
            if (this.running && !awaitingNextRtpBurst) {
              awaitingNextRtpBurst = true
              await this.injectInboundSilenceTail()
            }
            // Keep looping — server echo TTS on later turns resumes RTP on the same track.
            await new Promise((resolve) => setTimeout(resolve, 50))
            continue
          }
          if (isVoiceDebugEnabled()) {
            voiceDebugLog(MODULE, `readSample/processInboundPcm error: ${String(error)}`)
          }
          await new Promise((resolve) => setTimeout(resolve, 20))
        }
      }
    })()
  }
}
