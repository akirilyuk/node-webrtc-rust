#!/usr/bin/env node
/**
 * Download and extract a Sherpa-ONNX streaming Zipformer bundle.
 *
 * Run from repo root:
 *   npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-model:es --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --lang=zh
 *   npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --list
 *
 * Then export SHERPA_MODEL_PATH (and optionally SHERPA_LANGUAGE) before `npm run start`.
 */

import { existsSync, mkdirSync, readdirSync } from 'fs'
import { execFileSync } from 'child_process'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import {
  DEFAULT_MODEL_ID,
  getSherpaModelEntry,
  printSherpaModelCatalog,
  resolveSherpaModelId,
  SHERPA_ASR_RELEASE_BASE,
  SHERPA_MODEL_CATALOG,
} from './sherpa-model-catalog.mjs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const EXAMPLE_ROOT = join(__dirname, '..')
const MODELS_DIR = join(EXAMPLE_ROOT, '.models')

/** @deprecated Use DEFAULT_MODEL_ID from catalog */
export const DEFAULT_BUNDLE =
  SHERPA_MODEL_CATALOG.find((entry) => entry.id === DEFAULT_MODEL_ID)?.bundle ?? ''

const REQUIRED_KEYS = ['tokens.txt', 'encoder', 'decoder', 'joiner']

function parseArgs(argv) {
  let lang = process.env.SHERPA_LANGUAGE?.trim() ?? ''
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

  return { lang: resolveSherpaModelId(lang || DEFAULT_MODEL_ID), list }
}

function printUsage() {
  console.log(`Usage: node scripts/download-model.mjs [--lang=<id>] [--list]

Environment:
  SHERPA_LANGUAGE   Default language id when --lang is omitted (e.g. es, zh, de)

Examples:
  npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
  npm run download-model:fr --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
  npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa -- --lang=ru
`)
  printSherpaModelCatalog()
}

function bundleDir(bundle) {
  return join(MODELS_DIR, bundle)
}

function verifyBundle(dir) {
  const names = readdirSync(dir).map((n) => n.toLowerCase())
  for (const key of REQUIRED_KEYS) {
    const found = names.some((name) => {
      if (key === 'tokens.txt') return name === 'tokens.txt'
      return name.endsWith('.onnx') && name.includes(key)
    })
    if (!found) {
      throw new Error(`Missing ${key} artifact in ${dir}`)
    }
  }
}

function download(url, dest) {
  console.log(`Downloading ${url}`)
  execFileSync('curl', ['-fsSL', '-o', dest, url], { stdio: 'inherit' })
}

function printEnvHints(entry, targetDir) {
  console.log('\n✓ Sherpa model ready')
  console.log(`  Language: ${entry.label} (${entry.language ?? entry.id})`)
  if (entry.note) {
    console.log(`  Note: ${entry.note}`)
  }
  console.log(`\nexport SHERPA_MODEL_PATH="${targetDir}"`)
  if (entry.language) {
    console.log(`export SHERPA_LANGUAGE="${entry.language}"`)
  }
  console.log(
    '\nThen start the browser demo:\n  npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa',
  )
}

/**
 * @param {string} modelId
 */
export function downloadSherpaModel(modelId = DEFAULT_MODEL_ID) {
  const entry = getSherpaModelEntry(resolveSherpaModelId(modelId))
  if (!entry) {
    throw new Error(
      `Unknown language id "${modelId}". Run with --list to see supported ids: ${SHERPA_MODEL_CATALOG.map((e) => e.id).join(', ')}`,
    )
  }

  if (entry.kind !== 'transducer' || !entry.bundle) {
    throw new Error(`${entry.label} (${entry.id}): ${entry.note ?? 'No downloadable bundle for this example.'}`)
  }

  const targetDir = bundleDir(entry.bundle)
  if (existsSync(targetDir)) {
    verifyBundle(targetDir)
    console.log(`Model already present: ${targetDir}`)
    printEnvHints(entry, targetDir)
    return { entry, targetDir }
  }

  mkdirSync(MODELS_DIR, { recursive: true })
  const archivePath = join(MODELS_DIR, `${entry.bundle}.tar.bz2`)
  const url = `${SHERPA_ASR_RELEASE_BASE}/${entry.bundle}.tar.bz2`

  if (!existsSync(archivePath)) {
    if (entry.approxMb) {
      console.log(`Fetching ${entry.label} model (${entry.approxMb} MB compressed)…`)
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
    printSherpaModelCatalog()
    return
  }
  downloadSherpaModel(lang)
}

main()
