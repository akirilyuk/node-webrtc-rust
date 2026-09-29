/**
 * Headless CLI demo for mid-session STT swap helpers (no speech pools / Sherpa models).
 *
 * ## What this exercises
 *
 * - `VoiceAgent.updateStt` — queues a new `SttConfig` on mock vendors (same API as production).
 * - `VoiceAgent.replayLastUtterance` — re-decode path; here it correctly **fails** because no
 *   utterance was finalized and no PCM was buffered yet.
 *
 * ## Language-switch hosts (real sessions)
 *
 * After the user speaks and you change STT (new language or Sherpa model path):
 *
 * 1. `await agent.updateStt({ provider: 'local-sherpa', language: 'de', modelPath: '...' })`
 * 2. Wait for `stt_config_updated` (optional sanity check on `event.language`).
 * 3. `await agent.replayLastUtterance()` so the phrase that triggered the switch is transcribed
 *    again with the new recognizer — listen for `user_speech_final` with `replay: true`.
 * 4. Optionally `await agent.updateTts({ ... }, { cancelInflight: true })` before the next reply.
 *
 * `user_language` from LID does **not** perform steps 1–4 for you; a coordinator or your
 * `onSpeechEvent` handler must call `updateStt` / `updateTts` when that is your product policy.
 *
 * Full PCM + replay final: Rust `cargo test -p node-webrtc-rust-speech replay_last_utterance`.
 * API reference: `packages/sdk/VOICE-API.md` § Mid-session STT/TTS language and model switch.
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
