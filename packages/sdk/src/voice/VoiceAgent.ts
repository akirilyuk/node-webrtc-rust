import {
  JsEventDeliveryMode,
  JsSpeechEventType,
  JsSttVendor,
  JsTtsVendor,
  JsVadSampleRate,
  JsVoiceAgent as NativeVoiceAgent,
  type JsSpeechEvent,
  type JsSttConfig,
  type JsTtsConfig,
  type JsVadConfig,
  type JsVoiceAgent,
  type JsVoiceAgentConfig,
} from '@node-webrtc-rust/bindings'

import type { RemoteAudioTrack } from '../RemoteAudioTrack'
import { debugEvent, debugFn } from '../debug'
import { isVoiceDebugEnabled, voiceDebugLog } from './debug'
import type {
  EventDeliveryMode,
  SpeechEvent,
  SpeechEventListener,
  SpeechEventName,
  SpeechEventType,
  SttConfig,
  TtsConfig,
  VadConfig,
  VoiceAgentConfig,
  VoiceAttachOptions,
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
  }
}

function toJsTtsConfig(tts: TtsConfig): JsTtsConfig {
  return {
    provider: ttsVendorToJs(tts.provider),
    model: tts.model,
    modelPath: tts.modelPath,
    voice: tts.voice,
    apiKey: tts.apiKey,
    postUtteranceSilenceMs: tts.postUtteranceSilenceMs,
  }
}

function sttVendorToJs(vendor: SttConfig['provider']): JsSttVendor {
  switch (vendor) {
    case 'openai':
      return JsSttVendor.Openai
    case 'deepgram':
      return JsSttVendor.Deepgram
    case 'google':
      return JsSttVendor.Google
    case 'assemblyai':
      return JsSttVendor.Assemblyai
    case 'local-sherpa':
      return JsSttVendor.LocalSherpa
    default:
      return JsSttVendor.Mock
  }
}

function ttsVendorToJs(vendor: TtsConfig['provider']): JsTtsVendor {
  switch (vendor) {
    case 'openai':
      return JsTtsVendor.Openai
    case 'elevenlabs':
      return JsTtsVendor.Elevenlabs
    case 'google':
      return JsTtsVendor.Google
    case 'cartesia':
      return JsTtsVendor.Cartesia
    case 'local-sherpa':
      return JsTtsVendor.LocalSherpa
    default:
      return JsTtsVendor.Mock
  }
}

function toJsConfig(config?: VoiceAgentConfig): JsVoiceAgentConfig | undefined {
  if (!config) return undefined
  const postUtteranceSilenceMs =
    config.postUtteranceSilenceMs ?? config.tts?.postUtteranceSilenceMs
  return {
    vad: toJsVadConfig(config.vad),
    events: config.events?.mode ? { mode: eventModeToJs(config.events.mode) } : undefined,
    stt: config.stt ? toJsSttConfig(config.stt) : undefined,
    tts: config.tts ? toJsTtsConfig(config.tts) : undefined,
    postUtteranceSilenceMs,
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
  return {
    type: jsEventTypeToString(rawType ?? JsSpeechEventType.Error),
    text: event.text ?? undefined,
    error: event.error ?? undefined,
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
  async start(): Promise<void> {
    debugFn(MODULE, 'start')
    await this.native.start()
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
        yield fromJsSpeechEvent(event)
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
