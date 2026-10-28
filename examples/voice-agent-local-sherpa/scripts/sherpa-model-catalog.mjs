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

// ---------------------------------------------------------------------------
// Speech-enhancement (denoiser) bench assets (benchmarks/speaker/README.md). Test and bench only.
// Every file is checked against `checksum.txt` of its release. Licenses: GTCRN MIT, DPDFNet
// Apache-2.0.
// ---------------------------------------------------------------------------

export const SHERPA_ENHANCE_RELEASE_BASE =
  'https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models'

/**
 * `sha256` is set for files that the release's `checksum.txt` does not list (it lists only
 * gtcrn_simple.onnx and the sample wavs): the digest GitHub shows for that release asset
 * (`gh api repos/k2-fsa/sherpa-onnx/releases/tags/speech-enhancement-models`, 2026-10-28).
 */
export const SHERPA_ENHANCE_MODELS = [
  { id: 'gtcrn', file: 'gtcrn_simple.onnx' },
  {
    id: 'dpdfnet_baseline',
    file: 'dpdfnet_baseline.onnx',
    sha256: 'debaf0e8893479fca91b8b4c1eae8db195aa8980ccc9012f5809fde2b738151a',
  },
  {
    id: 'dpdfnet2',
    file: 'dpdfnet2.onnx',
    sha256: 'ce35d6025fc71df0ef10d1540e1b7916837bbfe5f6896deb744508d2cad487a9',
  },
  {
    id: 'dpdfnet4',
    file: 'dpdfnet4.onnx',
    sha256: '71b588bc26163941aa82a592cce924b08c1fbdc0879fe44f5a2d4eac44bd8420',
  },
  {
    id: 'dpdfnet8',
    file: 'dpdfnet8.onnx',
    sha256: '2751c1f5a4e849d23a07c675b4c838158b249b42152f10cc318522dd339134f0',
  },
  {
    id: 'dpdfnet2_48khz_hr',
    file: 'dpdfnet2_48khz_hr.onnx',
    sha256: '0b399f8a58dc4d70d8cd97541f5c39869406145193b957d00a03b66070944928',
  },
]
