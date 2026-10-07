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
import type {
  SpeechEvent,
  SpeechEventType,
  SttHoldMode,
  SttHoldOutcome,
} from './types'

/** Data channel label used by `examples/voice-agent-browser` and recommended for apps. */
export const VOICE_CONTROL_CHANNEL_LABEL = 'voice-control'

/** Optional high-frequency binary sync channel (see {@link VOICE_SYNC_CHANNEL_LABEL}). */
export const VOICE_SYNC_CHANNEL_LABEL = 'voicethere-sync'

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
  language?: string
  error?: string
  /** `language_id_skipped`: `too_short` | `deferred_tts` | `no_audio` | `undetermined`. */
  reason?: SpeechEvent['reason']
  /** `language_id_skipped`: buffered speech (ms). */
  speechMs?: number
  /** Shared across `user_speaking_start` … `user_speech_final` for one utterance. */
  utteranceId?: string
  /** True when this final was produced by a replay. */
  replay?: boolean
  /** Original utterance id when `replay` is true. */
  replacesUtteranceId?: string
  /** Set by a host when releasing a held final after a failed language switch. */
  languageMismatch?: boolean
  /** `stt_hold_started`: the hold mode. */
  holdMode?: SttHoldMode
  /** `stt_hold_ended`: how the hold ended. */
  holdOutcome?: SttHoldOutcome
  /** `stt_hold_started` / `stt_hold_ended`: PCM buffered (ms). */
  bufferedMs?: number
  /** `stt_hold_ended`: PCM dropped by the buffer bound or `first_utterance` (ms). */
  droppedMs?: number
  /** `agent_speaking_start`: speak request to first PCM chunk from the TTS vendor (ms). */
  firstChunkMs?: number
  /** `agent_speaking_start`: speak request to first outbound PCM frame (ms). */
  firstAudioMs?: number
  /** `tts_wait`: synthesis start waited this long on refusals (ms). */
  waitMs?: number
  /** `tts_wait`: refused attempts that were retried. */
  attempts?: number
}

/** Server → client: text queued for agent TTS (`ctx.speak` / `sendTextToTTS`). */
export interface VoiceControlAgentSpeakMessage {
  type: 'agent_speak'
  text: string
  ts?: string
}

export type VoiceControlClientMessage = VoiceControlSpeakMessage

export type VoiceControlServerMessage =
  VoiceControlSpeechEventMessage | VoiceControlAgentSpeakMessage

/** Maps agent TTS text to the wire format sent before playback starts. */
export function agentSpeakToControlMessage(
  text: string,
  options?: { ts?: string },
): VoiceControlAgentSpeakMessage {
  return {
    type: 'agent_speak',
    text,
    ts: options?.ts,
  }
}

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
    language: event.language,
    error: event.error,
    reason: event.reason,
    speechMs: event.speechMs,
    utteranceId: event.utteranceId,
    replay: event.replay,
    replacesUtteranceId: event.replacesUtteranceId,
    languageMismatch: event.languageMismatch,
    holdMode: event.holdMode,
    holdOutcome: event.holdOutcome,
    bufferedMs: event.bufferedMs,
    droppedMs: event.droppedMs,
    firstChunkMs: event.firstChunkMs,
    firstAudioMs: event.firstAudioMs,
    waitMs: event.waitMs,
    attempts: event.attempts,
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
  /** Called for binary payloads on the voice-control channel. */
  onDataChannelBinary?: (data: Buffer) => void | Promise<void>
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
  options?: {
    /** Return `false` to keep an event off the wire (default: forward every event). */
    filter?: (event: SpeechEvent) => boolean
  },
): () => void {
  let active = true

  void (async () => {
    for await (const event of agent.speechEvents()) {
      if (!active) {
        break
      }
      if (options?.filter && !options.filter(event)) {
        continue
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
    if (typeof event.data !== 'string') {
      const binary =
        event.data instanceof ArrayBuffer
          ? Buffer.from(event.data)
          : Buffer.isBuffer(event.data)
            ? event.data
            : Buffer.from(event.data as Uint8Array)
      if (options?.onDataChannelBinary) {
        void options.onDataChannelBinary(binary)
        return
      }
      if (options?.onDataChannelMessage) {
        void options.onDataChannelMessage(binary.toString('utf8'))
      }
      return
    }

    const message = parseVoiceControlClientMessage(event.data)
    if (!message) {
      if (options?.onDataChannelMessage) {
        void options.onDataChannelMessage(event.data)
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
