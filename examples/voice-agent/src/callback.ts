/**
 * VoiceAgent — **callback** event delivery demo (mock vendors, no API keys).
 *
 * ## What this shows
 *
 * - Constructing `VoiceAgent` with `events: { mode: 'callback' }`
 * - Registering handlers with `agent.on('user_speech_final', …)` etc.
 * - TTS injection via `sendTextToTTS()` → outbound track → user peer hears agent audio
 *
 * ## Why callback mode?
 *
 * Callback mode matches familiar EventEmitter patterns in Node apps. The native layer
 * invokes your JS handlers when speech events fire. Use this when you already have
 * an event-driven agent loop (e.g. connect handlers, then `start()`).
 *
 * For a single async loop over all events, see `start:stream` (`stream.ts`).
 *
 * ## Run
 *
 * ```bash
 * npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent
 * ```
 *
 * Expected: logs for mock TTS lifecycle; mock STT finals only appear after enough
 * inbound PCM (not exercised in this short script — see `start:live:*` for STT input).
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { createLoopbackAudio, mockVoiceConfig } from './shared-loopback.js'

async function main(): Promise<void> {
  // Two peer connections: agent (VoiceAgent host) and simulated user.
  const { agentOut, agentInbound, cleanup } = await createLoopbackAudio()

  const agent = new VoiceAgent({
    ...mockVoiceConfig,
    // Callback-only: speechEvents() still works but this demo uses on() handlers.
    events: { mode: 'callback' },
  })

  // Register before start() so no events are missed once the pipeline runs.
  agent.on('user_speech_final', (event) => {
    console.log('[callback] user_speech_final:', event.text)
  })
  agent.on('agent_speaking_start', () => {
    console.log('[callback] agent_speaking_start')
  })
  agent.on('barge_in', () => {
    console.log('[callback] barge_in')
  })

  // attach() binds one conversation: inbound user audio + outbound agent TTS track.
  await agent.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent.start()

  // Text → mock TTS vendor → PCM → agentOut → RTP → user peer.
  await agent.sendTextToTTS('Hello from the mock voice agent.')

  // Brief pause so async native TTS drain can finish before stop().
  await new Promise((resolve) => setTimeout(resolve, 500))

  await agent.stop()
  await cleanup()
  console.log('Voice callback demo complete')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
