/**
 * Echo voice handler — same semantics as e2e/fixtures/echo-agent and runner echo child:
 * - speak "ready" when a client peer connects
 * - speak `echo:{text}` on user_speech_final
 * - speak `echo:{text}` on voice-control chat messages (except voice-control probes)
 *
 * Used by {@link @node-webrtc-rust/example-voice-agent-echo-pod} (direct SessionPod layer,
 * no runner / no agent child IPC).
 */

import type { VoiceSessionContext, VoiceSessionHandler } from '@node-webrtc-rust/helpers'

import { isSpeechEventLogEnabled, logSpeakText, logSpeechEvent } from './speech-event-log.js'

/** Chat messages used for connectivity checks — must not trigger TTS echo. */
export function isVoiceControlProbeText(text: string): boolean {
  const probe = text.trim().toLowerCase()
  return probe === 'ping' || probe === 'health' || probe === 'healthz'
}

function parseChatText(payload: string): string | null {
  try {
    const message = JSON.parse(payload) as { type?: string; text?: string }
    if (message.type === 'chat' && typeof message.text === 'string') {
      return message.text
    }
    if (typeof message.text === 'string') return message.text
    return null
  } catch {
    return payload.trim() || null
  }
}

/** Notify client of TTS text before playback (works with published helpers — no session-host patch required). */
async function speakToClient(ctx: VoiceSessionContext, text: string): Promise<void> {
  ctx.sendToClient({ type: 'agent_speak', text })
  await ctx.speak(text)
}

export const echoVoiceHandler: VoiceSessionHandler = {
  async onPeerConnected(ctx) {
    if (isSpeechEventLogEnabled()) {
      console.error(`[echo-pod] onPeerConnected peer=${ctx.peerId} room=${ctx.roomId}`)
    }
    logSpeakText(`echo-pod/${ctx.peerId}`, 'ready')
    await speakToClient(ctx, 'ready')
  },

  async onSpeechEvent(ctx, event) {
    logSpeechEvent(`echo-pod/${ctx.peerId}`, event)
    if (event.type !== 'user_speech_final') return
    const trimmed = event.text?.trim()
    if (!trimmed) return
    const echoText = `echo:${trimmed}`
    logSpeakText(`echo-pod/${ctx.peerId}`, echoText)
    await speakToClient(ctx, echoText)
  },

  async onDataChannelMessage(ctx, payload) {
    const text = parseChatText(payload)
    if (!text?.trim()) return
    if (isVoiceControlProbeText(text)) return
    const echoText = `echo:${text.trim()}`
    if (isSpeechEventLogEnabled()) {
      console.error(
        `[echo-pod] chat echo peer=${ctx.peerId} text=${JSON.stringify(text.trim())}`,
      )
    }
    logSpeakText(`echo-pod/${ctx.peerId}`, echoText)
    await speakToClient(ctx, echoText)
  },
}
