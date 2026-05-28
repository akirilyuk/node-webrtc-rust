/**
 * VoiceAgent — **stream** event delivery demo (mock vendors, no API keys).
 *
 * ## What this shows
 *
 * - `events: { mode: 'stream' }` — consume everything via `for await (… of agent.speechEvents())`
 * - Same TTS path as callback mode; only the **event delivery** API differs
 *
 * ## Why stream mode?
 *
 * Stream mode fits LLM/agent frameworks that want one async iterator (or Node Readable)
 * feeding a state machine, rather than many `on()` registrations. Both modes share the
 * same native event bus; set `mode: 'both'` to use handlers and the iterator together.
 *
 * Under the hood the SDK polls `pullSpeechEvent()` from native code in a tight loop
 * while `running === true`.
 *
 * ## Run
 *
 * ```bash
 * npm run start:stream --workspace=@node-webrtc-rust/example-voice-agent
 * ```
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { createLoopbackAudio, mockVoiceConfig } from './shared-loopback.js'

async function main(): Promise<void> {
  const { agentOut, agentInbound, cleanup } = await createLoopbackAudio()

  const agent = new VoiceAgent({
    ...mockVoiceConfig,
    events: { mode: 'stream' },
  })

  await agent.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent.start()

  // Start consuming before TTS so agent_speaking_* events are not missed.
  const streamTask = (async () => {
    for await (const event of agent.speechEvents()) {
      console.log('[stream]', event.type, event.text ?? event.error ?? '')
      // Mock TTS is short; exit after playback ends to avoid polling forever.
      if (event.type === 'agent_speaking_end') break
    }
  })()

  await agent.sendTextToTTS('Streaming speech events from mock TTS.')

  // Race: stream may finish first, or we time out if no events (misconfiguration).
  await Promise.race([streamTask, new Promise((resolve) => setTimeout(resolve, 2000))])

  await agent.stop()
  await cleanup()
  console.log('Voice stream demo complete')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
