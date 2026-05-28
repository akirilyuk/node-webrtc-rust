/**
 * Sherpa-ONNX offline VITS/Piper TTS bundles for `local-sherpa` TTS.
 * Canonical data: examples/shared/sherpa-tts-model-catalog.json
 */

import catalog from '../../shared/sherpa-tts-model-catalog.json' with { type: 'json' }

export const SHERPA_TTS_RELEASE_BASE = catalog.releaseBase
export const DEFAULT_TTS_MODEL_ID = catalog.defaultModelId
export const SHERPA_TTS_MODEL_CATALOG = catalog.models

/** @param {string} id */
export function getSherpaTtsModelEntry(id) {
  const normalized = id.trim().toLowerCase()
  return SHERPA_TTS_MODEL_CATALOG.find((entry) => entry.id === normalized)
}

/** @param {string} [id] */
export function resolveSherpaTtsModelId(id) {
  if (!id || !id.trim()) {
    return DEFAULT_TTS_MODEL_ID
  }
  return id.trim().toLowerCase()
}

export function listSherpaTtsModelIds() {
  return SHERPA_TTS_MODEL_CATALOG.map((entry) => entry.id)
}

export function printSherpaTtsModelCatalog() {
  const ws = catalog.exampleWorkspace
  console.log('Sherpa offline TTS models for voice-agent-local-sherpa:\n')
  for (const entry of SHERPA_TTS_MODEL_CATALOG) {
    console.log(`  ${entry.id.padEnd(10)}  ${entry.label.padEnd(36)}  ${entry.bundle}`)
  }
  console.log(`\nDownload: npm run download-tts --workspace=${ws} -- --lang=en`)
  console.log(`Or:       npm run download-tts:en --workspace=${ws}`)
  console.log(`List:     npm run download-tts:list --workspace=${ws}`)
}
