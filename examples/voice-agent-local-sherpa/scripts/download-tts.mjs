#!/usr/bin/env node
/**
 * Download and extract a Sherpa-ONNX offline VITS/Piper TTS bundle.
 *
 * Run from repo root:
 *   npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-tts:en --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --lang=es
 *   npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --list
 *
 * Then export SHERPA_TTS_MODEL_PATH (and optionally SHERPA_TTS_SPEAKER) before `npm run start`.
 */

import { existsSync, mkdirSync, readdirSync } from 'fs'
import { execFileSync } from 'child_process'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import {
  DEFAULT_TTS_MODEL_ID,
  getSherpaTtsModelEntry,
  printSherpaTtsModelCatalog,
  resolveSherpaTtsModelId,
  SHERPA_TTS_MODEL_CATALOG,
  SHERPA_TTS_RELEASE_BASE,
} from './sherpa-tts-model-catalog.mjs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const EXAMPLE_ROOT = join(__dirname, '..')
const MODELS_DIR = join(EXAMPLE_ROOT, '.models')

function parseArgs(argv) {
  let lang = process.env.SHERPA_TTS_LANGUAGE?.trim() ?? ''
  let list = false

  for (const arg of argv) {
    if (arg === '--list' || arg === '-l') {
      list = true
      continue
    }
    if (arg.startsWith('--lang=')) {
      lang = arg.slice('--lang='.length)
      continue
    }
    if (arg === '--help' || arg === '-h') {
      printUsage()
      process.exit(0)
    }
    if (!arg.startsWith('-') && !lang) {
      lang = arg
    }
  }

  return { lang: resolveSherpaTtsModelId(lang || DEFAULT_TTS_MODEL_ID), list }
}

function printUsage() {
  console.log(`Usage: node scripts/download-tts.mjs [--lang=<id>] [--list]

Environment:
  SHERPA_TTS_LANGUAGE   Default voice bundle id when --lang is omitted (e.g. en, es, de)

Examples:
  npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
  npm run download-tts:de --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
`)
  printSherpaTtsModelCatalog()
}

function bundleDir(bundle) {
  return join(MODELS_DIR, bundle)
}

function verifyBundle(dir) {
  const names = readdirSync(dir).map((n) => n.toLowerCase())
  const hasTokens = names.includes('tokens.txt')
  const hasOnnx = names.some((name) => name.endsWith('.onnx'))
  const hasEspeak = existsSync(join(dir, 'espeak-ng-data'))
  if (!hasTokens || !hasOnnx || !hasEspeak) {
    throw new Error(`Missing TTS artifacts in ${dir} (need tokens.txt, *.onnx, espeak-ng-data/)`)
  }
}

function download(url, dest) {
  console.log(`Downloading ${url}`)
  execFileSync('curl', ['-fsSL', '-o', dest, url], { stdio: 'inherit' })
}

function printEnvHints(entry, targetDir) {
  console.log('\n✓ Sherpa TTS model ready')
  console.log(`  Voice: ${entry.label} (${entry.language ?? entry.id})`)
  console.log(`\nexport SHERPA_TTS_MODEL_PATH="${targetDir}"`)
  if (entry.speakerId !== undefined) {
    console.log(`export SHERPA_TTS_SPEAKER="${entry.speakerId}"`)
  }
  console.log(
    '\nPair with STT (npm run download-stt), then start:\n  npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa',
  )
}

/**
 * @param {string} modelId
 */
export function downloadSherpaTtsModel(modelId = DEFAULT_TTS_MODEL_ID) {
  const entry = getSherpaTtsModelEntry(resolveSherpaTtsModelId(modelId))
  if (!entry) {
    throw new Error(
      `Unknown TTS voice id "${modelId}". Run with --list: ${SHERPA_TTS_MODEL_CATALOG.map((e) => e.id).join(', ')}`,
    )
  }

  const targetDir = bundleDir(entry.bundle)
  if (existsSync(targetDir)) {
    verifyBundle(targetDir)
    console.log(`TTS model already present: ${targetDir}`)
    printEnvHints(entry, targetDir)
    return { entry, targetDir }
  }

  mkdirSync(MODELS_DIR, { recursive: true })
  const archivePath = join(MODELS_DIR, `${entry.bundle}.tar.bz2`)
  const url = `${SHERPA_TTS_RELEASE_BASE}/${entry.bundle}.tar.bz2`

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

  verifyBundle(targetDir)
  printEnvHints(entry, targetDir)
  return { entry, targetDir }
}

function main() {
  const { lang, list } = parseArgs(process.argv.slice(2))
  if (list) {
    printSherpaTtsModelCatalog()
    return
  }
  downloadSherpaTtsModel(lang)
}

main()
