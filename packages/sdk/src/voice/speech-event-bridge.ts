/**
 * Bridge VoiceAgent speech events and TTS control over an RTCDataChannel.
 *
 * Use this when the agent runs on a Node server and a browser (or other peer)
 * needs STT/VAD/barge-in notifications plus a way to request spoken output.
 */

import type { RTCDataChannel } from '../RTCDataChannel'
import type { MessageEvent } from '../types'
import type { VoiceAgent } from './VoiceAgent'
import { isVoiceDebugEnabled, voiceDebugLog } from './debug'
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
  /** ISO-8601 server time when the event was forwarded (multi-client event log). */
  ts?: string
  text?: string
  error?: string
}

export type VoiceControlClientMessage = VoiceControlSpeakMessage

export type VoiceControlServerMessage = VoiceControlSpeechEventMessage

/** Maps a {@link SpeechEvent} to the wire format sent on the voice-control data channel. */
export function speechEventToControlMessage(
  event: SpeechEvent,
  options?: { ts?: string },
): VoiceControlSpeechEventMessage {
  return {
    type: 'speech_event',
    event: event.type,
    ts: options?.ts,
    text: event.text,
    error: event.error,
  }
}

/**
 * Parses a client JSON message. Returns null if not `{ type: 'speak', text: string }`.
 */
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
  /** Called for other JSON payloads on the voice-control channel. */
  onDataChannelMessage?: (payload: string) => void | Promise<void>
}

function sendSpeechEventToChannel(channel: RTCDataChannel, event: SpeechEvent): void {
  if (channel.readyState !== 'open') {
    if (isVoiceDebugEnabled()) {
      voiceDebugLog('voice-control', `drop ${event.type} (channel state=${channel.readyState})`)
    }
    return
  }
  if (isVoiceDebugEnabled()) {
    const detail = event.text ? ` text=${JSON.stringify(event.text.slice(0, 120))}` : ''
    voiceDebugLog('voice-control', `send ${event.type}${detail}`)
  }
  channel.send(JSON.stringify(speechEventToControlMessage(event, { ts: new Date().toISOString() })))
}

/**
 * Forwards speech events from {@link VoiceAgent.speechEvents} to a data channel.
 * Call only **after** {@link VoiceAgent.start} — the pull stream is inactive until then.
 */
export function forwardVoiceAgentSpeechToDataChannel(
  agent: VoiceAgent,
  channel: RTCDataChannel,
): () => void {
  let active = true

  void (async () => {
    for await (const event of agent.speechEvents()) {
      if (!active) {
        break
      }
      sendSpeechEventToChannel(channel, event)
    }
  })()

  return () => {
    active = false
  }
}

/**
 * Handles inbound `{ type: 'speak' }` on a voice-control data channel.
 */
export function wireVoiceControlSpeakHandler(
  agent: VoiceAgent,
  channel: RTCDataChannel,
  options?: WireVoiceAgentToDataChannelOptions,
): () => void {
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
    if (!message) {
      if (options?.onDataChannelMessage) {
        void options.onDataChannelMessage(payload)
      }
      return
    }

    if (options?.onSpeak) {
      void options.onSpeak(message.text)
      return
    }
    void agent.sendTextToTTS(message.text)
  }

  return () => {
    channel.onmessage = previousOnMessage
  }
}

/**
 * Wires inbound `speak` handling on the data channel.
 *
 * Speech events are **not** forwarded here — call
 * {@link forwardVoiceAgentSpeechToDataChannel} after {@link VoiceAgent.start}.
 *
 * Returns an unsubscribe function — call it before closing the channel or agent.
 */
export function wireVoiceAgentToDataChannel(
  agent: VoiceAgent,
  channel: RTCDataChannel,
  options?: WireVoiceAgentToDataChannelOptions,
): () => void {
  return wireVoiceControlSpeakHandler(agent, channel, options)
}
