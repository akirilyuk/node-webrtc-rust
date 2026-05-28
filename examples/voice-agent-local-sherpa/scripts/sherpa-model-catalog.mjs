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
  console.log(`\nDownload: npm run download-model --workspace=${ws} -- --lang=es`)
  console.log(`Or:       npm run download-model:es --workspace=${ws}`)
  console.log(`List:     npm run download-model:list --workspace=${ws}`)
}
