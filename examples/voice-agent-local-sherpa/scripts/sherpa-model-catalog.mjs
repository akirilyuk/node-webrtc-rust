/**
 * Sherpa streaming Zipformer catalog for `local-sherpa`.
 * Canonical data: examples/shared/sherpa-local-model-catalog.json
 */

import catalog from '../../shared/sherpa-local-model-catalog.json' with { type: 'json' }

export const SHERPA_ASR_RELEASE_BASE = catalog.releaseBase
export const DEFAULT_MODEL_ID = catalog.defaultModelId
export const SHERPA_MODEL_CATALOG = catalog.models

/** @param {string} id */
export function getSherpaModelEntry(id) {
  const normalized = id.trim().toLowerCase()
  return SHERPA_MODEL_CATALOG.find((entry) => entry.id === normalized)
}

/** @param {string} [id] */
export function resolveSherpaModelId(id) {
  if (!id || !id.trim()) {
    return DEFAULT_MODEL_ID
  }
  return id.trim().toLowerCase()
}

export function listSherpaModelIds() {
  return SHERPA_MODEL_CATALOG.map((entry) => entry.id)
}

export function printSherpaModelCatalog() {
  const ws = catalog.exampleWorkspace
  console.log('Sherpa streaming models for voice-agent-local-sherpa:\n')
  for (const entry of SHERPA_MODEL_CATALOG) {
    const status = entry.kind === 'transducer' ? entry.bundle : 'unavailable'
    console.log(`  ${entry.id.padEnd(4)}  ${entry.label.padEnd(22)}  ${status}`)
    if (entry.note) {
      console.log(`       ${entry.note}`)
    }
  }
  console.log(`\nDownload: npm run download-stt --workspace=${ws} -- --lang=es`)
  console.log(`Or:       npm run download-stt:es --workspace=${ws}`)
  console.log(`List:     npm run download-stt:list --workspace=${ws}`)
}

// ---------------------------------------------------------------------------
// Speaker-ID bench assets (benchmarks/speaker/README.md). Test and bench only: nothing here is
// shipped in an image. Every file is checked against `checksum.txt` of its release.
// ---------------------------------------------------------------------------

export const SHERPA_SPEAKER_RELEASE_BASE =
  'https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models' // sic

/**
 * Speaker-embedding candidates that passed the license check (README table). `nemo_en_titanet_small`
 * is kept because its NGC model card says the license is that of the NeMo Toolkit (Apache-2.0).
 */
export const SHERPA_SPEAKER_MODELS = [
  { id: '3dspeaker-campplus-en', file: '3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx' },
  { id: 'wespeaker-resnet34-lm-en', file: 'wespeaker_en_voxceleb_resnet34_LM.onnx' },
  { id: 'nemo-titanet-small-en', file: 'nemo_en_titanet_small.onnx' },
]

/** Real-voice clips from the same release (enroll `*-sr-*`, test `*-test-sr-*`). */
export const SHERPA_SPEAKER_WAVS = [
  'fangjun-sr-1.wav',
  'fangjun-sr-2.wav',
  'fangjun-test-sr-1.wav',
  'leijun-sr-1.wav',
  'leijun-sr-2.wav',
  'leijun-test-sr-1.wav',
  'liudehua-sr-1.wav',
  'liudehua-test-sr-1.wav',
]

/** Piper bundles for the other (background) voices; amy-low (main voice) comes from download-tts. */
export const SHERPA_SPEAKER_PIPER_BUNDLES = [
  'vits-piper-en_US-ryan-medium',
  'vits-piper-en_US-kristin-medium',
  'vits-piper-en_US-hfc_male-medium',
  'vits-piper-en_US-joe-medium',
  'vits-piper-en_US-hfc_female-medium',
  'vits-piper-en_US-lessac-medium',
  'vits-piper-en_US-libritts_r-medium',
]
