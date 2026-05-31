/**
 * ═══════════════════════════════════════════════════════════════════════════
 *  YOUR VOICE AGENT LOGIC — edit this file only
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * Each browser tab gets its own {@link VoiceSessionContext} (`peerId` like `client-tab1`).
 * Use `ctx.speak(text)` to play TTS on that tab's outbound audio track.
 *
 * Events still appear in the browser event log (mirrored over `voice-control`).
 */

import type { VoiceSessionHandler } from '@node-webrtc-rust/helpers'

export const voiceHandler: VoiceSessionHandler = {
  /**
   * Called when the pipeline detects speech activity or STT transcripts.
   * Add your LLM, tools, or state machine here.
   */
  async onSpeechEvent(ctx, event) {
    switch (event.type) {
      case 'user_speech_partial':
        // Optional: stream partials to your UI or LLM
        break

      case 'user_speech_final': {
        const heard = event.text?.trim()
        if (!heard) break
        console.log(`[${ctx.peerId}] user said: ${heard}`)
        // ── Paste your logic below ──────────────────────────────────────
        await ctx.speak(`You said: ${heard}`)
        // ── e.g. await ctx.speak(await myLlm(heard)) ───────────────────
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

  /**
   * Called when the user submits the Speak form in the browser.
   * Omit this handler to use default TTS (speak the text as-is).
   */
  async onSpeakRequest(ctx, text) {
    console.log(`[${ctx.peerId}] speak form: ${text}`)
    await ctx.speak(text)
  },
}
