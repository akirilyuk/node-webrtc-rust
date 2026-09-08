#!/usr/bin/env node
/**
 * Download Sherpa-ONNX Whisper tiny for spoken language identification (encoder + decoder only).
 *
 *   npm run download-lid --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-lid:whisper-tiny --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Then export SHERPA_LID_MODEL_PATH before enabling `languageId.modelPath` on VoiceAgent.
 */

import { existsSync, mkdirSync, readdirSync } from 'fs'
import { execFileSync } from 'child_process'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import {
  getSherpaModelEntry,
  resolveSherpaModelId,
  SHERPA_ASR_RELEASE_BASE,
} from './sherpa-model-catalog.mjs'

const DEFAULT_MODEL_ID = 'whisper-tiny-lid'

const __dirname = dirname(fileURLToPath(import.meta.url))
const EXAMPLE_ROOT = join(__dirname, '..')
const MODELS_DIR = join(EXAMPLE_ROOT, '.models')

const ENCODER_KEYS = [
  'tiny-encoder.int8.onnx',
  'encoder.int8.onnx',
  'tiny-encoder.onnx',
  'encoder.onnx',
]
const DECODER_KEYS = [
  'tiny-decoder.int8.onnx',
  'decoder.int8.onnx',
  'tiny-decoder.onnx',
  'decoder.onnx',
]

function bundleDir(bundle) {
  return join(MODELS_DIR, bundle)
}

function verifyLidBundle(dir) {
  const names = readdirSync(dir).map((n) => n.toLowerCase())
  const hasEncoder = ENCODER_KEYS.some((key) => names.includes(key.toLowerCase()))
  const hasDecoder = DECODER_KEYS.some((key) => names.includes(key.toLowerCase()))
  if (!hasEncoder || !hasDecoder) {
    throw new Error(
      `Missing Whisper encoder/decoder ONNX in ${dir} (expected tiny-encoder*.onnx and tiny-decoder*.onnx)`,
    )
  }
}

function download(url, dest) {
  console.log(`Downloading ${url}`)
  execFileSync('curl', ['-fsSL', '-o', dest, url], { stdio: 'inherit' })
}

function printEnvHints(targetDir) {
  console.log('\n✓ Sherpa spoken-language ID model ready')
  console.log(`\nexport SHERPA_LID_MODEL_PATH="${targetDir}"`)
  console.log(
    '\nEnable on VoiceAgent: languageId: { modelPath: process.env.SHERPA_LID_MODEL_PATH }',
  )
}

/**
 * @param {string} [modelId]
 */
export function downloadSherpaLidModel(modelId = DEFAULT_MODEL_ID) {
  const entry = getSherpaModelEntry(resolveSherpaModelId(modelId))
  if (!entry?.bundle || entry.kind !== 'whisper-lid') {
    throw new Error(
      `Unknown LID model id "${modelId}". Use whisper-tiny-lid (see sherpa-local-model-catalog.json).`,
    )
  }

  const targetDir = bundleDir(entry.bundle)
  if (existsSync(targetDir)) {
    verifyLidBundle(targetDir)
    console.log(`LID model already present: ${targetDir}`)
    printEnvHints(targetDir)
    return { entry, targetDir }
  }

  mkdirSync(MODELS_DIR, { recursive: true })
  const archivePath = join(MODELS_DIR, `${entry.bundle}.tar.bz2`)
  const url = `${SHERPA_ASR_RELEASE_BASE}/${entry.bundle}.tar.bz2`

  if (!existsSync(archivePath)) {
    if (entry.approxMb) {
      console.log(`Fetching ${entry.label} (${entry.approxMb} MB compressed)…`)
    }
    download(url, archivePath)
  }

  console.log(`Extracting ${archivePath} …`)
  execFileSync('tar', ['-xjf', archivePath, '-C', MODELS_DIR], { stdio: 'inherit' })

  if (!existsSync(targetDir)) {
    throw new Error(`Expected extracted directory ${targetDir} — check archive layout`)
  }

  verifyLidBundle(targetDir)
  printEnvHints(targetDir)
  return { entry, targetDir }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const modelId = process.argv[2]?.trim() || DEFAULT_MODEL_ID
  downloadSherpaLidModel(modelId)
}
