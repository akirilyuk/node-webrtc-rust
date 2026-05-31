/**
 * Hooks for app logic on top of {@link VoiceAgentSessionHost}.
 *
 * Use {@link VoiceSessionContext.speak} to play TTS on the agent outbound track.
 */

import type { VoiceAgent, SpeechEvent } from '@node-webrtc-rust/sdk/voice'

/** Per-browser-tab context passed into your handlers. */
export interface VoiceSessionContext {
  /** Signaling peer id (e.g. `client-tab1`). */
  peerId: string
  /** This tab's VoiceAgent — advanced use only; prefer {@link VoiceSessionContext.speak}. */
  agent: VoiceAgent
  /** Synthesize `text` and stream audio to the browser. */
  speak: (text: string) => Promise<void>
}

/**
 * Implement this in your app (see `examples/voice-agent-local-sherpa-multi-client/src/voice-handler.ts`).
 */
export interface VoiceSessionHandler {
  /**
   * Called for each pipeline event: VAD (`user_speaking_*`), STT (`user_speech_*`),
   * TTS lifecycle (`agent_speaking_*`), `barge_in`, `error`, etc.
   *
   * Events are still mirrored to the browser `voice-control` channel for the demo UI
   * unless you customize the host.
   */
  onSpeechEvent?: (ctx: VoiceSessionContext, event: SpeechEvent) => void | Promise<void>

  /**
   * Called when the browser sends `{ type: 'speak', text }` (Speak form).
   * Omit to use default behavior (`agent.sendTextToTTS(text)`).
   */
  onSpeakRequest?: (ctx: VoiceSessionContext, text: string) => void | Promise<void>
}
