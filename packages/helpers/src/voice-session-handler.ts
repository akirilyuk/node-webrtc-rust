/**
 * Hooks for app logic on top of {@link VoiceAgentSessionHost}.
 *
 * Use {@link VoiceSessionContext.speak} to play TTS on the agent outbound track.
 */

import type { VoiceAgent, SendTextToTtsOptions, SpeechEvent } from '@node-webrtc-rust/sdk/voice'

/** Which WebRTC data channel carried a binary payload. */
export type DataChannelKind = 'control' | 'sync'

/** Per-browser-tab context passed into your handlers. */
export interface VoiceSessionContext {
  /** Signaling peer id (e.g. `client-tab1`). */
  peerId: string
  /** Orchestrator / signaling room id (set by {@link SessionPod}). */
  roomId?: string
  /** This tab's VoiceAgent — omitted in data-only mode. */
  agent?: VoiceAgent
  /** Synthesize `text` and stream audio to the browser. */
  speak: (text: string, options?: SendTextToTtsOptions) => Promise<void>
  /** Send a JSON payload to the browser over the voice-control data channel. */
  sendToClient: (payload: unknown) => void
  /**
   * Wait for pending voice-control SCTP data to flush before tearing down WebRTC.
   * Returns false when the channel closes or flush times out.
   */
  flushToClient?: () => Promise<boolean>
  /** Send raw bytes — prefers the sync channel when negotiated and open. */
  sendBinaryToClient: (data: Buffer | Uint8Array, channel?: DataChannelKind) => void
}

/**
 * Implement this in your app (see `examples/voice-agent-local-sherpa-multi-client/src/voice-handler.ts`).
 */
export interface VoiceSessionHandler {
  /** Called when a browser peer's WebRTC session is ready (voice: after VoiceAgent start). */
  onPeerConnected?: (ctx: VoiceSessionContext) => void | Promise<void>

  /** Called when a browser peer disconnects (before teardown). */
  onPeerDisconnected?: (ctx: VoiceSessionContext) => void | Promise<void>

  /**
   * Called when signaling/WebRTC setup started but billable connect never completed
   * (peer-left or transport closed before {@link onPeerConnected}).
   */
  onPeerSignalingLost?: (ctx: VoiceSessionContext) => void | Promise<void>

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
   * Use {@link VoiceSessionContext.speak} for this tab only. Omit to default TTS on `ctx`.
   */
  onSpeakRequest?: (ctx: VoiceSessionContext, text: string) => void | Promise<void>

  /**
   * Called for non-speak data channel JSON (e.g. `{ type: 'chat', text }`).
   * Raw string is passed when JSON parsing is deferred to the handler.
   */
  onDataChannelMessage?: (ctx: VoiceSessionContext, payload: string) => void | Promise<void>

  /**
   * Called for binary payloads on the voice-control or sync data channel.
   */
  onDataChannelBinary?: (
    ctx: VoiceSessionContext,
    data: Buffer,
    channel: DataChannelKind,
  ) => void | Promise<void>

  /**
   * Called only for an explicit broadcast command (e.g. `POST /api/broadcast-speak`).
   * Use `contexts` to TTS every connected tab — never use this path for STT replies.
   * Return peer ids that received audio. Omit for host default (`sendTextToTTS` on each).
   */
  onBroadcastSpeak?: (
    text: string,
    contexts: readonly VoiceSessionContext[],
  ) => string[] | Promise<string[]>
}
