/**
 * Bridge VoiceAgent speech events and TTS control over an RTCDataChannel.
 *
 * Use this when the agent runs on a Node server and a browser (or other peer)
 * needs STT/VAD/barge-in notifications plus a way to request spoken output.
 */

import type { RTCDataChannel } from '../RTCDataChannel'
import type { MessageEvent } from '../types'
import type { VoiceAgent } from './VoiceAgent'
import type { SpeechEvent, SpeechEventType } from './types'

/** Data channel label used by `examples/voice-agent-browser` and recommended for apps. */
export const VOICE_CONTROL_CHANNEL_LABEL = 'voice-control'

/** Client → server: request TTS playback on the agent outbound track. */
export interface VoiceControlSpeakMessage {
  type: 'speak'
  text: string
}

/** Server → client: mirror of {@link SpeechEvent} from the native pipeline. */
export interface VoiceControlSpeechEventMessage {
  type: 'speech_event'
  event: SpeechEventType
  text?: string
  error?: string
}

export type VoiceControlClientMessage = VoiceControlSpeakMessage

export type VoiceControlServerMessage = VoiceControlSpeechEventMessage

export function speechEventToControlMessage(event: SpeechEvent): VoiceControlSpeechEventMessage {
  return {
    type: 'speech_event',
    event: event.type,
    text: event.text,
    error: event.error,
  }
}

export function parseVoiceControlClientMessage(raw: string): VoiceControlSpeakMessage | null {
  try {
    const parsed = JSON.parse(raw) as Partial<VoiceControlSpeakMessage>
    if (parsed.type === 'speak' && typeof parsed.text === 'string' && parsed.text.length > 0) {
      return { type: 'speak', text: parsed.text }
    }
  } catch {
    // ignore malformed payloads
  }
  return null
}

export interface WireVoiceAgentToDataChannelOptions {
  /** Called when the client requests TTS via `{ type: 'speak', text }`. */
  onSpeak?: (text: string) => void | Promise<void>
}

/**
 * Forwards all {@link VoiceAgent} speech events to the remote peer as JSON and
 * handles inbound `speak` messages by calling {@link VoiceAgent.sendTextToTTS}.
 *
 * Returns an unsubscribe function — call it before closing the channel or agent.
 */
export function wireVoiceAgentToDataChannel(
  agent: VoiceAgent,
  channel: RTCDataChannel,
  options?: WireVoiceAgentToDataChannelOptions,
): () => void {
  const sendEvent = (event: SpeechEvent) => {
    if (channel.readyState !== 'open') return
    channel.send(JSON.stringify(speechEventToControlMessage(event)))
  }

  const onSpeech: (event: SpeechEvent) => void = (event) => {
    sendEvent(event)
  }

  agent.on('speech', onSpeech)

  const previousOnMessage = channel.onmessage
  channel.onmessage = (event: MessageEvent) => {
    previousOnMessage?.(event)
    const payload =
      typeof event.data === 'string'
        ? event.data
        : event.data instanceof ArrayBuffer
          ? Buffer.from(event.data).toString('utf8')
          : String(event.data)

    const message = parseVoiceControlClientMessage(payload)
    if (!message) return

    if (options?.onSpeak) {
      void options.onSpeak(message.text)
      return
    }
    void agent.sendTextToTTS(message.text)
  }

  return () => {
    agent.off('speech', onSpeech)
    channel.onmessage = previousOnMessage
  }
}
