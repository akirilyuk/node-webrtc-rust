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
} from './types'

const MODULE = 'voice::VoiceAgent'

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
  return {
    vad: toJsVadConfig(config.vad),
    events: config.events?.mode ? { mode: eventModeToJs(config.events.mode) } : undefined,
    stt: config.stt ? toJsSttConfig(config.stt) : undefined,
    tts: config.tts ? toJsTtsConfig(config.tts) : undefined,
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
    case JsSpeechEventType.BargeIn:
      return 'barge_in'
    default:
      return 'error'
  }
}

/**
 * Voice agent orchestrating VAD, STT, TTS, and barge-in for one WebRTC session.
 */
export class VoiceAgent {
  private readonly native: JsVoiceAgent
  private inboundTrack?: RemoteAudioTrack
  private inboundLoop?: Promise<void>
  private running = false
  private readonly listeners = new Map<SpeechEventName, Set<SpeechEventListener>>()

  constructor(config?: VoiceAgentConfig) {
    debugFn(MODULE, 'constructor')
    this.native = new NativeVoiceAgent(toJsConfig(config))
    this.native.setOnSpeechEvent((event) => {
      if (!event) return
      const mapped = fromJsSpeechEvent(event)
      this.dispatch(mapped)
    })
  }

  /**
   * Attaches inbound/outbound audio tracks for one peer connection session.
   */
  async attach(options: VoiceAttachOptions): Promise<void> {
    debugFn(MODULE, 'attach')
    this.inboundTrack = options.inboundTrack
    await this.native.attach(options.outboundTrack.native)
  }

  async start(): Promise<void> {
    debugFn(MODULE, 'start')
    await this.native.start()
    this.running = true
    voiceDebugLog(MODULE, 'native start() complete — starting inbound PCM loop')
    this.startInboundLoop()
  }

  async stop(): Promise<void> {
    debugFn(MODULE, 'stop')
    this.running = false
    await this.native.stop()
  }

  async sendTextToTTS(text: string): Promise<void> {
    debugFn(MODULE, 'sendTextToTTS', `chars=${text.length}`)
    await this.native.sendTextToTts(text)
  }

  async flushTts(): Promise<void> {
    debugFn(MODULE, 'flushTts')
    await this.native.flushTts()
  }

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
   * Async iterator over speech events (stream delivery mode).
   */
  async *speechEvents(): AsyncGenerator<SpeechEvent, void, undefined> {
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

  private startInboundLoop(): void {
    const track = this.inboundTrack
    if (!track) return

    this.inboundLoop = (async () => {
      let frameCount = 0
      while (this.running) {
        try {
          const pcm = await track.readSample()
          frameCount += 1
          if (isVoiceDebugEnabled() && (frameCount === 1 || frameCount % 50 === 0)) {
            voiceDebugLog(MODULE, `readSample frame=${frameCount} bytes=${pcm.length}`)
          }
          if (isVoiceDebugEnabled() && (frameCount === 1 || frameCount % 50 === 0)) {
            voiceDebugLog(MODULE, `processInboundPcm begin frame=${frameCount}`)
          }
          await this.native.processInboundPcm(pcm, 20)
          if (isVoiceDebugEnabled() && (frameCount === 1 || frameCount % 50 === 0)) {
            voiceDebugLog(MODULE, `processInboundPcm done frame=${frameCount}`)
          }
        } catch (error: unknown) {
          if (isVoiceDebugEnabled()) {
            voiceDebugLog(MODULE, `readSample/processInboundPcm error: ${String(error)}`)
          }
          await new Promise((resolve) => setTimeout(resolve, 20))
        }
      }
    })()
  }
}
