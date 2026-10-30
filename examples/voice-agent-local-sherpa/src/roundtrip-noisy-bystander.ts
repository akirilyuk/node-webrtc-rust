/**
 * E2E noisy bystander (Sherpa STT + TTS) — adaptive VAD + speaker gate over a real WebRTC loopback.
 *
 * One client leg plays a generated "conversation" test scene (`speaker_fixtures`, never committed):
 * two enrollment utterances of the caller, then alternating caller and bystander turns at least
 * 2.5 s apart. The bystander's voice is 15 dB below the caller (`--bystander-db -15`), except the
 * first two bystander turns: long sentences at 6 dB below (`--long-bystanders 2
 * --long-bystander-db -6`), loud enough to open the adaptive VAD over the babble, so each pass has
 * bystander finals for the gate to label other. The background babble is at the scene level, 15 dB
 * below the caller. No babble sentence starts
 * within 1 s before a caller turn (`--babble-clear-ms 1000`): other speech that starts less than
 * about 1 s before the caller can fall into the 1.5 s check and make it reject the caller, so the
 * barge-in is suppressed and the final is labelled other. The agent answers every
 * `user_speech_final` with a long reply, like an app, so every turn starts during playback.
 * `barge_in` is paired with turns by `utteranceId`: each turn takes the first `user_speaking_start`
 * inside its scene slot, everything else goes by id.
 *
 * Two passes, each with a fresh agent:
 *   1. caller at the default level (-20 dBFS);
 *   2. quiet caller at -26 dBFS (`_l26` scene), bystander and babble levels still relative to the caller.
 *
 * Per pass the script asserts:
 *   - no `barge_in` carries a bystander utterance's id, and its final (when detected) has `speaker: 'other'`;
 *   - a bystander final `unknown` with `speakerUnknownReason: 'short'` (less speech than
 *     minCheckSpeechMs) is accepted with its barge-in; any other unknown fails; at least 1
 *     bystander turn per pass must be `other` without a barge-in (anti-vacuous);
 *   - every caller turn barges in, its final has `speaker: 'caller'`, and qualified phrases match
 *     the manifest text by the roundtrip oracle (`wordSimilarity`; long sentences are not judged);
 *   - the barge-in comes at most 300 ms after the later of speech start + checkSpeechMs and the
 *     first partial with >= minSttPartialTokens tokens on that utterance; at most 3 s after start.
 *
 * Needs the speaker assets (`npm run download-speaker`) and the scenes:
 *   cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
 *     --models .models/speaker --main-tts .models/vits-piper-en_US-amy-low \
 *     --phrases stt-phrase-qualification.json --out .models/speaker-fixtures --main-levels -26 \
 *     --min-turn-gap-ms 2500 --bystander-db -15 --babble-clear-ms 1000 --babble-clear-pull \
 *     --long-bystanders 2 --long-bystander-db -6 --only s3_test_m15
 * (CI does this in `scripts/ci/ensure-speaker-fixtures.sh`.)
 *
 * Run:
 *   npm run start:roundtrip-noisy-bystander --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 * Env: SHERPA_SPEAKER_SCENES_DIR (scene dir), SHERPA_SPEAKER_MODEL_PATH (embedding model).
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
  BYSTANDER_RELATIVE_DB,
  QUIET_CALLER_DBFS,
  WARMUP_S,
  evaluateTurns,
  matchTurnUtterances,
  MIN_STT_PARTIAL_TOKENS,
  readWavPcm,
  type Seen,
  type Turn,
} from './roundtrip-noisy-bystander-helpers.js'
import { streamSilence } from './pcm-relay.js'
import { resolveRoundtripVoiceConfig } from './resolve-voice-config.js'
import { exitSherpaRoundtripFailure } from './roundtrip-failure-debug.js'
import { logRoundtripSpeechEvent } from './roundtrip-speech-events.js'
import { logRoundtripScriptBanner } from './roundtrip-topology-log.js'

const here = dirname(fileURLToPath(import.meta.url))
const exampleRoot = join(here, '..')

/**
 * Agent reply long enough to still be playing past the next caller turn's latest barge bound
 * (the next turn's qualifying partial came up to ~7.7 s after the previous final; the old
 * one-sentence reply drained after ~7.66 s and lost the race). Measured Piper reply: 18.0-18.6 s
 * (`tts synthesis done audio_duration_ms`, 18 replies, run 20261029-145736).
 */
const AGENT_PHRASE =
  'The quick brown fox jumps over the lazy dog and then keeps on talking about the weather in the valley. ' +
  'After that it describes the long road home, the small bridge over the river, and the old house on the hill. ' +
  'Finally it reads out a short list of groceries so the caller has plenty of time to interrupt the playback.'

interface ManifestSegment {
  start_ms: number
  end_ms: number
  role: 'main' | 'bystander' | 'babble'
  text: string
  kind: 'qualified' | 'long' | 'background'
  enroll?: boolean
}

interface ManifestScene {
  id: string
  kind: string
  set: string
  level_db: number
  main_db?: number
  wav48: string
  segments: ManifestSegment[]
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

function pickScene(manifest: { scenes: ManifestScene[] }, quiet: boolean): ManifestScene {
  const scene = manifest.scenes.find(
    (s) =>
      s.kind === 'conversation' &&
      s.set === 'test' &&
      s.level_db === BYSTANDER_RELATIVE_DB &&
      (quiet ? s.main_db === QUIET_CALLER_DBFS : s.main_db === undefined),
  )
  if (scene == null) {
    throw new Error(
      `no test-set conversation scene at level ${BYSTANDER_RELATIVE_DB}${quiet ? ` and main ${QUIET_CALLER_DBFS} dBFS` : ''} in manifest.json — run speaker_fixtures with --main-levels ${QUIET_CALLER_DBFS}`,
    )
  }
  return scene
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
    speakerGate: { enabled: true, modelPath },
  }
}

async function runPass(params: {
  label: string
  scene: ManifestScene
  scenesDir: string
  config: VoiceAgentConfig
  modelPath: string
}): Promise<string[]> {
  const { label, scene } = params
  const turns: Turn[] = scene.segments
    .filter((s) => (s.role === 'main' || s.role === 'bystander') && s.enroll !== true)
    .map((s) => ({
      role: s.role as 'main' | 'bystander',
      startMs: s.start_ms,
      endMs: s.end_ms,
      text: s.text,
      kind: s.kind,
    }))
  const pcm = readWavPcm(join(params.scenesDir, scene.wav48))
  console.log(`--- ${label}: scene ${scene.id}, ${turns.length} judged turns ---`)

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
        // Like an app: answer every final with a reply long enough to still be playing when the
        // next turn starts, so every caller and bystander turn starts during playback.
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
        })
      }
    } catch (error) {
      console.error('[speech-events] stream error:', error)
    }
  })()
  void drainInbound(userInbound, () => running)

  anchorEpoch = Date.now()
  await playPaced(userOut, pcm)
  await streamSilence(userOut, 3)
  running = false
  await listener.stop().catch(() => undefined)
  await cleanup().catch(() => undefined)
  // Diagnostics: scene clock vs the agent's first user_speaking_start per turn.
  matchTurnUtterances(turns, seen).forEach((start, i) => {
    const firstStart = start?.atMs
    console.log(
      `${label}: lag turn=${i} sceneStart=${turns[i]!.startMs} firstStart=${firstStart ?? 'none'} lagMs=${firstStart != null ? firstStart - turns[i]!.startMs : 'none'}`,
    )
  })
  return evaluateTurns(turns, seen, label)
}

async function main(): Promise<void> {
  installRoundtripWallClockTimeout(420_000)
  const scenesDir =
    process.env.SHERPA_SPEAKER_SCENES_DIR?.trim() ||
    join(exampleRoot, '.models', 'speaker-fixtures')
  const modelPath =
    process.env.SHERPA_SPEAKER_MODEL_PATH?.trim() ||
    join(exampleRoot, '.models', 'speaker', 'nemo_en_titanet_small.onnx')
  const manifestPath = join(scenesDir, 'manifest.json')
  if (!existsSync(manifestPath) || !existsSync(modelPath)) {
    throw new Error(
      `missing ${!existsSync(manifestPath) ? manifestPath : modelPath} — run download-speaker and speaker_fixtures (see file header)`,
    )
  }
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as { scenes: ManifestScene[] }
  const { config, label } = resolveRoundtripVoiceConfig()

  console.log('=== Sherpa noisy bystander E2E ===')
  logRoundtripScriptBanner({
    script: 'roundtrip-noisy-bystander',
    pipeline: label,
    extra: [
      `vad=adaptive speakerGate=on model=${modelPath}`,
      `bystander ${BYSTANDER_RELATIVE_DB} dB re caller; quiet pass ${QUIET_CALLER_DBFS} dBFS`,
    ],
  })

  const failures: string[] = []
  failures.push(
    ...(await runPass({
      label: 'Pass 1 (caller -20 dBFS)',
      scene: pickScene(manifest, false),
      scenesDir,
      config,
      modelPath,
    })),
  )
  failures.push(
    ...(await runPass({
      label: `Pass 2 (quiet caller ${QUIET_CALLER_DBFS} dBFS)`,
      scene: pickScene(manifest, true),
      scenesDir,
      config,
      modelPath,
    })),
  )

  if (failures.length > 0) {
    exitSherpaRoundtripFailure({ reason: 'noisy bystander assertions failed', failures })
  }
  console.log('\nNoisy bystander E2E OK — bystander ignored, caller (also quiet) barged in.')
  process.exit(0)
}

main().catch((error: unknown) => {
  exitSherpaRoundtripFailure({ reason: 'uncaught error', error })
})
