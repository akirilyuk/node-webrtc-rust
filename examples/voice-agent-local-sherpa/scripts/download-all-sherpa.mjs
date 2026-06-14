#!/usr/bin/env node
/**
 * Download every Sherpa STT/TTS bundle offered in the platform voice catalog
 * (same ids as GET /api/v1/voice/models and deploy make bm-sherpa-models).
 *
 *   npm run download-all --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-all:minimal --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Models land under examples/voice-agent-local-sherpa/.models/<bundle>/
 */

import { SHERPA_MODEL_CATALOG } from './sherpa-model-catalog.mjs'
import { SHERPA_TTS_MODEL_CATALOG } from './sherpa-tts-model-catalog.mjs'
import { downloadSherpaSttModel } from './download-stt.mjs'
import { downloadSherpaTtsModel } from './download-tts.mjs'

function parseArgs(argv) {
  let minimal = false
  for (const arg of argv) {
    if (arg === '--minimal') minimal = true
    if (arg === '--help' || arg === '-h') {
      console.log(`Usage: node scripts/download-all-sherpa.mjs [--minimal]

  --minimal   English STT + English TTS only (same as CI default)
  default     All downloadable catalog models (dedupes shared STT bundles)
`)
      process.exit(0)
    }
  }
  return { minimal }
}

function uniqueByBundle(catalog, kind, ids) {
  const want = ids ? new Set(ids) : null
  const seen = new Set()
  const out = []
  for (const m of catalog) {
    if (m.kind !== kind || !m.bundle) continue
    if (want && !want.has(m.id)) continue
    if (seen.has(m.bundle)) continue
    seen.add(m.bundle)
    out.push(m.id)
  }
  return out
}

const { minimal } = parseArgs(process.argv.slice(2))

const sttIds = minimal ? ['en'] : uniqueByBundle(SHERPA_MODEL_CATALOG, 'transducer')
const ttsIds = minimal ? ['en'] : uniqueByBundle(SHERPA_TTS_MODEL_CATALOG, 'vits')

console.log(`download-all-sherpa: ${sttIds.length} STT bundle(s), ${ttsIds.length} TTS bundle(s)\n`)

let failed = 0
for (const id of sttIds) {
  try {
    console.log(`\n=== STT ${id} ===`)
    downloadSherpaSttModel(id)
  } catch (err) {
    failed += 1
    console.error(`STT ${id} failed:`, err instanceof Error ? err.message : err)
  }
}
for (const id of ttsIds) {
  try {
    console.log(`\n=== TTS ${id} ===`)
    downloadSherpaTtsModel(id)
  } catch (err) {
    failed += 1
    console.error(`TTS ${id} failed:`, err instanceof Error ? err.message : err)
  }
}

console.log('\n--- Default env (English Kroko + Amy) ---')
console.log('  source scripts/export-sherpa-local-models.sh')
console.log('  # or: bash scripts/make-sherpa.sh --print-env')
console.log(
  '\nOther voices: set SHERPA_STT_BUNDLE / SHERPA_TTS_BUNDLE to a bundle dir under .models/',
)

if (failed > 0) {
  process.exit(1)
}
