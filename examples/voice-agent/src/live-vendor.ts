/**
 * Live vendor manual test — one preset per supported cloud provider.
 *
 * Usage:
 *   npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent
 *
 * Requires credentials in env (see examples/voice-agent/README.md).
 * Live HTTP/WS in vendor crates may still be stubbed until `--features live` is enabled
 * in native builds; errors will name the vendor and missing wiring.
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

  agent.on('user_speaking_start', () => console.log('[event] user_speaking_start'))
  agent.on('user_speaking_end', () => console.log('[event] user_speaking_end'))
  agent.on('user_speech_partial', (e) => console.log('[event] user_speech_partial:', e.text))
  agent.on('user_speech_final', (e) => console.log('[event] user_speech_final:', e.text))
  agent.on('agent_speaking_start', () => console.log('[event] agent_speaking_start'))
  agent.on('agent_speaking_end', () => console.log('[event] agent_speaking_end'))
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
