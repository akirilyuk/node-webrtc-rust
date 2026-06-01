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

export const voiceHandler: VoiceSessionHandler = {
  /**
   * Per-tab pipeline events. Always reply with `ctx.speak` — never broadcast here.
   */
  async onSpeechEvent(ctx, event) {
    switch (event.type) {
      case 'user_speech_partial':
        break

      case 'user_speech_final': {
        const heard = event.text?.trim()
        if (!heard) break
        console.log(`[${ctx.peerId}] user said: ${heard}`)
        await ctx.speak(`You said: ${heard}`)
        break
      }

      case 'user_speaking_start':
      case 'user_speaking_end':
      case 'agent_speaking_start':
      case 'agent_speaking_end':
      case 'barge_in':
      case 'error':
        break
    }
  },

  /** Per-tab Speak form — this tab only. */
  async onSpeakRequest(ctx, text) {
    console.log(`[${ctx.peerId}] speak form: ${text}`)
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
      console.log(`[${ctx.peerId}] broadcast: "${trimmed.slice(0, 80)}"`)
    }
    return spoken
  },
}
