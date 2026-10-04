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
 * ## Slow swaps (cold STT pool): STT hold
 *
 * When the new recognizer needs seconds to come up, use the hold instead of steps 1–3 so the old
 * model emits nothing wrong and speech that continues during the wait is not lost:
 *
 * 1. `await agent.beginSttHold({ mode: 'buffer_replay' })` — old STT is no longer fed or polled
 *    (`stt_hold_started`, counts only).
 * 2. Start the pool, then `await agent.updateStt({ ... })`.
 * 3. `await agent.releaseSttHold({ replay: true })` — held audio is decoded by the new STT
 *    (`user_speech_final` with `replay: true`), then live audio follows; or
 *    `await agent.cancelSttHold()` when the pool never came up.
 *
 * This headless demo has no tracks, so it can only show the not-running rejection; the audio path is
 * covered by `cargo test -p node-webrtc-rust-speech --test stt_hold_test`.
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

  try {
    await agent.beginSttHold({ mode: 'buffer_replay', maxBufferMs: 45_000 })
    console.error('expected beginSttHold to fail on an agent that is not running')
    process.exitCode = 1
    return
  } catch {
    console.log('beginSttHold correctly rejected before start()')
  }
  try {
    await agent.releaseSttHold({ replay: true })
    console.error('expected releaseSttHold to fail without an active hold')
    process.exitCode = 1
    return
  } catch {
    console.log('releaseSttHold correctly rejected without an active hold')
  }

  console.log('updateStt + replayLastUtterance + STT hold SDK path OK')
}

main().catch((err) => {
  console.error(err)
  process.exitCode = 1
})
