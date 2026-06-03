/**
 * ═══════════════════════════════════════════════════════════════════════════
 *  YOUR VOICE AGENT LOGIC — edit this file only
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * Each browser tab gets its own `VoiceSessionContext` (`peerId` like `client-tab1`) from helpers.
 *
 * **Routing rules (multi-client demo):**
 * - STT / per-tab Speak form → `ctx.speak(...)` only (that tab hears TTS).
 * - Page “Speak to all” / `POST /api/broadcast-speak` → {@link onBroadcastSpeak} only.
 */

import type { VoiceSessionHandler } from '@node-webrtc-rust/helpers'

/** Per-tab: ignore STT finals while agent TTS is playing (echo into Sherpa). */
const agentSpeakingByPeer = new Map<string, boolean>()

function logPeer(peerId: string, message: string): void {
  const ts = new Date().toISOString().slice(11, 23)
  console.log(`${ts} [${peerId}] ${message}`)
}

export const voiceHandler: VoiceSessionHandler = {
  /**
   * Per-tab pipeline events. Always reply with `ctx.speak` — never broadcast here.
   */
  async onSpeechEvent(ctx, event) {
    switch (event.type) {
      case 'user_speech_partial':
        break

      case 'agent_speaking_start':
        agentSpeakingByPeer.set(ctx.peerId, true)
        break

      case 'agent_speaking_end':
        agentSpeakingByPeer.set(ctx.peerId, false)
        break

      case 'user_speech_final': {
        if (agentSpeakingByPeer.get(ctx.peerId)) {
          logPeer(
            ctx.peerId,
            `ignored user_speech_final during agent TTS (likely echo): ${event.text?.slice(0, 60)}`,
          )
          break
        }
        const heard = event.text?.trim()
        if (!heard) break
        logPeer(ctx.peerId, `user said: ${heard}`)
        void ctx.speak(`You said: ${heard}`)
        break
      }

      case 'user_speaking_start':
      case 'user_speaking_end':
      case 'barge_in':
        // Rust clears playback on barge-in; ensure we accept the next user_speech_final.
        agentSpeakingByPeer.set(ctx.peerId, false)
        break

      case 'error':
        break
    }
  },

  /** Per-tab Speak form — this tab only. */
  async onSpeakRequest(ctx, text) {
    logPeer(ctx.peerId, `speak form: ${text}`)
    await ctx.speak(text)
  },

  /**
   * Explicit broadcast only (page button or API). Not used for STT replies.
   */
  async onBroadcastSpeak(text, contexts) {
    const trimmed = text.trim()
    if (!trimmed) return []

    const spoken: string[] = []
    for (const ctx of contexts) {
      await ctx.speak(trimmed)
      spoken.push(ctx.peerId)
      logPeer(ctx.peerId, `broadcast: "${trimmed.slice(0, 80)}"`)
    }
    return spoken
  },
}
