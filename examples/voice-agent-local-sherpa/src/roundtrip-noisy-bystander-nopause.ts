/**
 * E2E noisy bystander, no pause (Sherpa STT + TTS): a bystander talks, and the caller starts
 * 0-250 ms after the bystander's last word, over a babble bed that keeps the adaptive VAD open.
 * The agent runs with `speakerGate.splitOnSpeakerChange: true`, so it must split the bystander's
 * utterance where the caller starts (`user_utterance_split`, `from: 'other'`, `to: 'caller'`) and
 * give the caller a final with the caller's words only.
 *
 * Scenes: `speaker_fixtures --other-to-caller 8 --only s4_test_n` (`s4_test_nNN`, never committed;
 * CI: `scripts/ci/ensure-speaker-fixtures.sh nopause`). Each scene: a caller enrollment sentence,
 * 1.5 s, a bystander long sentence at 0 or -6 dB re the caller, then the caller (a long sentence or
 * a qualified phrase). One fresh agent per scene, so the enrollment sentence is the agent's first
 * caller speech; the harness inserts 2.5 s of silence after the enrollment sentence so that
 * utterance ends and enrolls (the gate enrolls after a final). Scenes whose bystander voice the gate cannot separate from the caller (pair FAR
 * >= 5 % in crates/vendor-sherpa-onnx/examples/FALSE_ACCEPT_BENCH.md, decision l) are skipped.
 *
 * Per scene the oracle (`evaluateNoPauseScene`, pairing by id only) fails on:
 *   - a barge_in on an utterance whose final is `other`;
 *   - no caller utterance of its own, a caller final not labelled `caller`, no barge_in on it, or
 *     a qualified caller phrase below the similarity threshold;
 *   - any `caller` final with bystander words (cut-point leak) and any leading glue;
 * and reports a leak after a gate false accept (split more than two check windows before the
 * caller's onset) as its own class without failing. At least one other→caller split per run.
 *
 * Run:
 *   npm run start:roundtrip-noisy-bystander-nopause --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 * Env: SHERPA_SPEAKER_NOPAUSE_SCENES_DIR (scene dir), SHERPA_SPEAKER_MODEL_PATH (embedding model),
 * SHERPA_NOPAUSE_ONLY (comma list of scene ids to play, e.g. s4_test_n02; debugging only).
 *
 * One writer per track: the scene PCM is the only writer on the client leg, paced from an anchor
 * (frame n is due at anchor + n x 20 ms), never `write(); sleep(20)`.
 */

import { existsSync, readFileSync } from 'fs'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import type { LocalAudioTrack, RemoteAudioTrack } from '@node-webrtc-rust/sdk'
import {
  VoiceAgent,
  VOICE_AGENT_VAD_PRESET,
  SPEECH_EVENT_TYPE,
  type VoiceAgentConfig,
} from '@node-webrtc-rust/sdk/voice'

import { PCM_FRAME_DURATION_MS, PCM_FULL_FRAME_BYTES } from '../../shared/pcm-streaming.js'
import { createBidirectionalLoopback } from '../../voice-agent/src/shared-loopback.js'
import { installRoundtripWallClockTimeout } from './roundtrip-counting.js'
import {
  MIN_STT_PARTIAL_TOKENS,
  WARMUP_S,
  readWavPcm,
  type Seen,
} from './roundtrip-noisy-bystander-helpers.js'
import {
  ENROLL_PAUSE_MS,
  benchVoiceLabel,
  enrollPauseAtMs,
  evaluateNoPauseScene,
  isExcludedBystander,
  parseSceneFilter,
  sceneSelected,
  shiftSegmentsAfter,
  type NoPauseLeak,
  type NoPauseSegment,
} from './roundtrip-noisy-bystander-nopause-helpers.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import { logRoundtripScriptBanner } from './roundtrip-topology-log.js'

const here = dirname(fileURLToPath(import.meta.url))
const exampleRoot = join(here, '..')

/** Same long reply as roundtrip-noisy-bystander.ts: still playing when the caller starts. */
const AGENT_PHRASE =
  'The quick brown fox jumps over the lazy dog and then keeps on talking about the weather in the valley. ' +
  'After that it describes the long road home, the small bridge over the river, and the old house on the hill. ' +
  'Finally it reads out a short list of groceries so the caller has plenty of time to interrupt the playback.'

interface ManifestSegment {
  start_ms: number
  end_ms: number
  role: 'main' | 'bystander' | 'babble'
  voice?: string
  sid?: number
  text: string
  kind: 'qualified' | 'long' | 'background'
  enroll?: boolean
  voiced_start_ms?: number
  voiced_end_ms?: number
}

interface ManifestScene {
  id: string
  kind: string
  wav48: string
  segments: ManifestSegment[]
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/** Plays `pcm` as 20 ms frames, frame n due at anchor + n x 20 ms. */
async function playPaced(sink: LocalAudioTrack, pcm: Buffer): Promise<void> {
  const anchorPerf = performance.now()
  const frames = Math.floor(pcm.length / PCM_FULL_FRAME_BYTES)
  for (let i = 0; i < frames; i++) {
    const wait = anchorPerf + i * PCM_FRAME_DURATION_MS - performance.now()
    if (wait > 0) await sleep(wait)
    await sink.writeSample(
      pcm.subarray(i * PCM_FULL_FRAME_BYTES, (i + 1) * PCM_FULL_FRAME_BYTES),
      PCM_FRAME_DURATION_MS,
    )
  }
}

async function drainInbound(track: RemoteAudioTrack, running: () => boolean): Promise<void> {
  while (running()) {
    try {
      const pcm = await track.readSample()
      if (pcm.length === 0) await sleep(10)
    } catch {
      await sleep(20)
    }
  }
}

/** Same agent as roundtrip-noisy-bystander.ts (adaptive VAD, STT-gated barge-in, gate + split on). */
function listenerConfig(base: VoiceAgentConfig, modelPath: string): VoiceAgentConfig {
  return {
    stt: base.stt,
    tts: base.tts,
    events: { mode: 'stream' },
    vad: {
      ...VOICE_AGENT_VAD_PRESET,
      ...base.vad,
      provider: 'adaptive',
      gateStt: true,
      bargeIn: {
        ...VOICE_AGENT_VAD_PRESET.bargeIn,
        ...base.vad?.bargeIn,
        enabled: true,
        useVad: true,
        flushTts: true,
        requireSttPartial: true,
        minSttPartialTokens: MIN_STT_PARTIAL_TOKENS,
        agentPlaybackGuardMs: 0,
      },
    },
    speakerGate: { enabled: true, modelPath, splitOnSpeakerChange: true },
  }
}

function toSegments(scene: ManifestScene): NoPauseSegment[] {
  return scene.segments
    .filter((s) => s.role === 'main' || s.role === 'bystander')
    .map((s) => ({
      role: s.role as 'main' | 'bystander',
      startMs: s.start_ms,
      endMs: s.end_ms,
      voicedStartMs: s.voiced_start_ms,
      voicedEndMs: s.voiced_end_ms,
      text: s.text,
      kind: s.kind,
      enroll: s.enroll,
      voice: s.voice,
      sid: s.sid,
    }))
}

interface SceneOutcome {
  failures: string[]
  otherToCallerSplits: number
  gateFalseAcceptLeaks: NoPauseLeak[]
  cutPointLeaks: number
  leadingGlue: number
}

async function runScene(params: {
  scene: ManifestScene
  scenesDir: string
  config: VoiceAgentConfig
  modelPath: string
}): Promise<SceneOutcome> {
  const { scene } = params
  const label = `Scene ${scene.id}`
  const sceneSegments = toSegments(scene)
  const scenePcm = readWavPcm(join(params.scenesDir, scene.wav48))
  // The gate enrolls only after a final, and the bed keeps the VAD open from the enrollment
  // sentence to the end of the scene: insert ENROLL_PAUSE_MS of silence after the enrollment
  // sentence (frame-aligned) and move every later segment by the same amount.
  const pauseAtMs = enrollPauseAtMs(sceneSegments)
  if (pauseAtMs == null) throw new Error(`${scene.id}: no enrollment segment`)
  const pauseAtFrame = Math.round(pauseAtMs / PCM_FRAME_DURATION_MS)
  const pauseFrames = Math.round(ENROLL_PAUSE_MS / PCM_FRAME_DURATION_MS)
  const cut = Math.min(pauseAtFrame * PCM_FULL_FRAME_BYTES, scenePcm.length)
  const pcm = Buffer.concat([
    scenePcm.subarray(0, cut),
    Buffer.alloc(pauseFrames * PCM_FULL_FRAME_BYTES),
    scenePcm.subarray(cut),
  ])
  const segments = shiftSegmentsAfter(
    sceneSegments,
    pauseAtFrame * PCM_FRAME_DURATION_MS,
    pauseFrames * PCM_FRAME_DURATION_MS,
  )
  const by = segments.find((s) => s.role === 'bystander')
  console.log(
    `--- ${label}: bystander ${by != null ? benchVoiceLabel(by.voice ?? '?', by.sid ?? 0) : '?'}, ${segments.length} segments ---`,
  )

  const { agentPc, userPc, userInbound, userOut, agentInbound, agentOut, cleanup } =
    await createBidirectionalLoopback()
  void agentPc
  void userPc
  const listener = new VoiceAgent(listenerConfig(params.config, params.modelPath))
  await listener.attach({ inboundTrack: agentInbound, outboundTrack: agentOut })
  await listener.start()
  await streamSilence(agentOut, WARMUP_S)
  await streamSilence(userOut, WARMUP_S)

  let anchorEpoch = Date.now()
  const seen: Seen[] = []
  let running = true
  void (async () => {
    try {
      for await (const event of listener.speechEvents()) {
        logRoundtripSpeechEvent(label, event)
        if (event.type === SPEECH_EVENT_TYPE.userSpeechFinal) {
          void listener.sendTextToTTS(AGENT_PHRASE).catch(() => undefined)
        }
        seen.push({
          type: event.type,
          atMs: Date.now() - anchorEpoch,
          text: event.text,
          utteranceId: event.utteranceId,
          speaker: event.speaker,
          speakerUnknownReason: event.speakerUnknownReason,
          speakerRechecked: event.speakerRechecked,
          speakerCheckWindows: event.speakerCheckWindows,
          fromUtteranceId: event.fromUtteranceId,
          toUtteranceId: event.toUtteranceId,
          splitAtMs: event.splitAtMs,
          fromStartMs: event.fromStartMs,
          from: event.from,
          to: event.to,
        })
      }
    } catch (error) {
      console.error('[speech-events] stream error:', error)
    }
  })()
  void drainInbound(userInbound, () => running)

  const sceneStartMediaMs = await listener.inboundMediaMs()
  anchorEpoch = Date.now()
  await playPaced(userOut, pcm)
  await streamSilence(userOut, 3)
  running = false
  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)
  const r = evaluateNoPauseScene(segments, seen, label, sceneStartMediaMs)
  return {
    failures: r.failures,
    otherToCallerSplits: r.otherToCallerSplits,
    gateFalseAcceptLeaks: r.gateFalseAcceptLeaks,
    cutPointLeaks: r.cutPointLeaks.length,
    leadingGlue: r.leadingGlue.length,
  }
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(420_000)
  const scenesDir =
    process.env.SHERPA_SPEAKER_NOPAUSE_SCENES_DIR?.trim() ||
    join(exampleRoot, '.models', 'speaker-fixtures-nopause')
  const modelPath =
    process.env.SHERPA_SPEAKER_MODEL_PATH?.trim() ||
    join(exampleRoot, '.models', 'speaker', 'nemo_en_titanet_small.onnx')
  const manifestPath = join(scenesDir, 'manifest.json')
  if (!existsSync(manifestPath) || !existsSync(modelPath)) {
    throw new Error(
      `missing ${!existsSync(manifestPath) ? manifestPath : modelPath} — run download-speaker and speaker_fixtures --other-to-caller (see file header)`,
    )
  }
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as { scenes: ManifestScene[] }
  const all = manifest.scenes.filter((s) => s.kind === 'other_to_caller' && /^s4_test_n/.test(s.id))
  const only = parseSceneFilter(process.env.SHERPA_NOPAUSE_ONLY)
  const scenes = all.filter((s) => {
    if (!sceneSelected(s.id, only)) return false
    const by = s.segments.find((g) => g.role === 'bystander')
    const skip = isExcludedBystander(by?.voice, by?.sid)
    if (skip) {
      console.log(
        `skip ${s.id}: bystander ${benchVoiceLabel(by?.voice ?? '?', by?.sid ?? 0)} is inseparable from the caller (pair FAR >= 5 %, FALSE_ACCEPT_BENCH.md)`,
      )
    }
    return !skip
  })
  if (scenes.length === 0) throw new Error(`no playable no-pause scene in ${manifestPath}`)
  const { config, label } = resolveRoundtripVoiceConfig()

  console.log('=== Sherpa noisy bystander E2E, no pause (other → caller split) ===')
  logRoundtripScriptBanner({
    script: 'roundtrip-noisy-bystander-nopause',
    pipeline: label,
    extra: [
      `vad=adaptive speakerGate=on splitOnSpeakerChange=on model=${modelPath}`,
      `scenes: ${scenes.map((s) => s.id).join(', ')} (${all.length - scenes.length} skipped)`,
    ],
  })

  const failures: string[] = []
  let splits = 0
  let cut = 0
  let glue = 0
  const falseAccepts: NoPauseLeak[] = []
  for (const scene of scenes) {
    const r = await runScene({ scene, scenesDir, config, modelPath })
    failures.push(...r.failures)
    splits += r.otherToCallerSplits
    cut += r.cutPointLeaks
    glue += r.leadingGlue
    falseAccepts.push(...r.gateFalseAcceptLeaks)
  }
  console.log(
    `no-pause summary: scenes ${scenes.length}; other→caller splits: ${splits}; cut-point leaks: ${cut}; leading glue finals: ${glue}; gate false-accept leaks (not failing): ${falseAccepts.length}`,
  )
  if (splits === 0) {
    failures.push('no other→caller split in any scene (vacuous pass)')
  }
  if (failures.length > 0) {
    exitSherpaRoundtripFailure({ reason: 'noisy bystander no-pause assertions failed', failures })
  }
  console.log('\nNoisy bystander no-pause E2E OK — caller finals hold only caller words.')
  process.exit(0)
}

main().catch((error: unknown) => {
  exitSherpaRoundtripFailure({ reason: 'uncaught error', error })
})
