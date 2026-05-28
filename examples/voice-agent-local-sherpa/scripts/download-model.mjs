#!/usr/bin/env node
/**
 * Download and extract the recommended English streaming Zipformer bundle for Sherpa-ONNX.
 *
 * Run from repo root:
 *   npm run download-model --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Then export SHERPA_MODEL_PATH to the printed directory before `npm run start`.
 */

import { existsSync, mkdirSync, readdirSync } from 'fs'
import { execFileSync } from 'child_process'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const EXAMPLE_ROOT = join(__dirname, '..')
const MODELS_DIR = join(EXAMPLE_ROOT, '.models')

/** Pinned bundle — small English streaming Zipformer (~70 MB compressed). */
export const DEFAULT_BUNDLE = 'sherpa-onnx-streaming-zipformer-en-2023-06-26'
const DOWNLOAD_URL = `https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${DEFAULT_BUNDLE}.tar.bz2`

const REQUIRED_KEYS = ['tokens.txt', 'encoder', 'decoder', 'joiner']

function bundleDir() {
  return join(MODELS_DIR, DEFAULT_BUNDLE)
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

function main() {
  const targetDir = bundleDir()
  if (existsSync(targetDir)) {
    verifyBundle(targetDir)
    console.log(`Model already present: ${targetDir}`)
    console.log(`\nexport SHERPA_MODEL_PATH="${targetDir}"`)
    return
  }

  mkdirSync(MODELS_DIR, { recursive: true })
  const archivePath = join(MODELS_DIR, `${DEFAULT_BUNDLE}.tar.bz2`)

  if (!existsSync(archivePath)) {
    download(DOWNLOAD_URL, archivePath)
  }

  console.log(`Extracting ${archivePath} …`)
  execFileSync('tar', ['-xjf', archivePath, '-C', MODELS_DIR], { stdio: 'inherit' })

  if (!existsSync(targetDir)) {
    throw new Error(`Expected extracted directory ${targetDir} — check archive layout`)
  }

  verifyBundle(targetDir)

  console.log('\n✓ Sherpa model ready')
  console.log(`\nexport SHERPA_MODEL_PATH="${targetDir}"`)
  console.log(
    '\nThen start the browser demo:\n  npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa',
  )
}

main()
