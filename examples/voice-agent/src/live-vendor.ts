/**
 * Live cloud vendor manual test — one entry point, six npm scripts.
 *
 * ## Purpose
 *
 * Validates your API keys and the full VoiceAgent attach → start → TTS/STT path against
 * real vendor configs **before** wiring a browser or telephony leg. Each vendor has its
 * own npm script so you can iterate on credentials independently.
 *
 * ## Run (pick one)
 *
 * ```bash
 * OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent
 * DEEPGRAM_API_KEY=... OPENAI_API_KEY=sk-... npm run start:live:deepgram --workspace=...
 * ```
 *
 * See `examples/voice-agent/README.md` for all env vars.
 *
 * ## Demo sequence (why each step exists)
 *
 * 1. **Validate env** — fail fast with a clear list of missing keys (never log key values).
 * 2. **Bidirectional loopback** — agent and user PCs so TTS goes out on `agentOut` and
 *    STT/VAD can read `agentInbound` fed by `userOut`.
 * 3. **Stream user tone (3 s)** — gives VAD/STT non-silent PCM without a microphone; mock
 *    STT uses byte counts, cloud STT needs real audio shape.
 * 4. **sendTextToTTS** — exercises the configured TTS vendor and outbound WebRTC track.
 * 5. **Log all event types** — compare VAD (`user_speaking_*`) vs STT (`user_speech_*`).
 *
 * ## Live API wiring
 *
 * Vendor HTTP/WebSocket calls live in Rust `vendor-*` crates. Default CI builds use stub
 * adapters; if TTS fails with "requires `--features live`", rebuild native with live
 * features enabled for that vendor (see crate README / follow-ups).
 *
 * ## argv vs env
 *
 * npm scripts pass the vendor id as `process.argv[2]`. You can also set `VOICE_VENDOR=openai`.
 */

import { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import {
  getLiveVendorPreset,
  listLiveVendorIds,
  missingEnvVars,
  type LiveVendorId,
} from '../../shared/voice-vendor-presets.js'
import { createBidirectionalLoopback, streamUserTone } from './shared-loopback.js'

async function main(): Promise<void> {
  const vendorId = (process.argv[2] ?? process.env.VOICE_VENDOR ?? '') as LiveVendorId
  const preset = getLiveVendorPreset(vendorId)

  if (!preset) {
    console.error(`Unknown vendor "${vendorId}". Supported: ${listLiveVendorIds().join(', ')}`)
    process.exit(1)
  }

  const missing = missingEnvVars(preset)
  if (missing.length > 0) {
    console.error(`Missing required env for ${preset.label}: ${missing.join(', ')}`)
    console.error(preset.notes)
    process.exit(1)
  }

  console.log(`=== VoiceAgent live demo: ${preset.label} ===`)
  console.log(preset.notes)
  console.log('STT provider:', preset.config.stt?.provider)
  console.log('TTS provider:', preset.config.tts?.provider)

  const { agentOut, agentInbound, userOut, cleanup } = await createBidirectionalLoopback()

  const agent = new VoiceAgent(preset.config)

  // Log the full event taxonomy — helps map Rust pipeline stages to your agent code.
  agent.on('user_speaking_start', () => console.log('[event] user_speaking_start (VAD)'))
  agent.on('user_speaking_end', () => console.log('[event] user_speaking_end (VAD)'))
  agent.on('user_speech_partial', (e) =>
    console.log('[event] user_speech_partial (STT):', e.text),
  )
  agent.on('user_speech_final', (e) => console.log('[event] user_speech_final (STT):', e.text))
  agent.on('agent_speaking_start', () => console.log('[event] agent_speaking_start (TTS)'))
  agent.on('agent_speaking_end', () => console.log('[event] agent_speaking_end (TTS)'))
  agent.on('barge_in', () => console.log('[event] barge_in'))
  agent.on('error', (e) => console.error('[event] error:', e.error))

  await agent.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await agent.start()

  console.log('Streaming user-side tone for STT/VAD (3s)…')
  await streamUserTone(userOut, 3)

  console.log('Sending TTS phrase…')
  try {
    await agent.sendTextToTTS(preset.ttsPhrase)
    console.log('TTS injection completed without error.')
  } catch (err) {
    // Stub adapters error here until live features are built — attach/loopback still validated.
    console.error('TTS injection failed (expected if live vendor wiring is not built yet):')
    console.error(err)
  }

  await new Promise((resolve) => setTimeout(resolve, 1000))
  await agent.stop()
  await cleanup()
  console.log('Live vendor demo finished.')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
