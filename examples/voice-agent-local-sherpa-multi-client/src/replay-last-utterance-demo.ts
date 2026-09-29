/**
 * Headless CLI demo for mid-session STT swap helpers (no speech pools / Sherpa models).
 *
 * Demonstrates `VoiceAgent.updateStt` and `VoiceAgent.replayLastUtterance` on mock vendors.
 * A full PCM utterance + replay final is covered by Rust integration tests
 * (`cargo test -p node-webrtc-rust-speech replay_last_utterance`).
 *
 * Run:
 *   npm run start:replay-last-utterance --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

async function main(): Promise<void> {
  const agent = new VoiceAgent({
    stt: { provider: 'mock', language: 'en' },
    tts: { provider: 'mock' },
  })

  await agent.updateStt({ provider: 'mock', language: 'de' })

  try {
    await agent.replayLastUtterance()
    console.error('expected replayLastUtterance to fail without a finalized utterance')
    process.exitCode = 1
    return
  } catch {
    console.log('replayLastUtterance correctly rejected with no buffered utterance')
  }

  console.log('updateStt + replayLastUtterance SDK path OK')
}

main().catch((err) => {
  console.error(err)
  process.exitCode = 1
})
