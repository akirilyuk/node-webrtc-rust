#!/usr/bin/env node
/**
 * Download the denoiser models of the enhancement bench into `.models/enhance/` (test and bench
 * only):
 *
 *   npm run download-enhance --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Files come from release `speech-enhancement-models` of k2-fsa/sherpa-onnx (gtcrn_simple,
 * dpdfnet_baseline, dpdfnet2, dpdfnet4, dpdfnet8, dpdfnet2_48khz_hr), each sha256-checked: against
 * that release's checksum.txt where it lists the file (gtcrn_simple.onnx), else against the hash
 * pinned in sherpa-model-catalog.mjs. A file that is already present with a matching hash is skipped. A
 * checksum mismatch deletes nothing that was already there and exits 1 naming the file.
 */

import { createHash } from 'crypto'
import { execFileSync } from 'child_process'
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync } from 'fs'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import { SHERPA_ENHANCE_MODELS, SHERPA_ENHANCE_RELEASE_BASE } from './sherpa-model-catalog.mjs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const ENHANCE_DIR = join(__dirname, '..', '.models', 'enhance')

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

/** `checksum.txt` of a release: "<name>\t<sha256>" per line. */
function fetchChecksums(base) {
  const out = execFileSync('curl', ['-fsSL', '--retry', '3', `${base}/checksum.txt`], {
    encoding: 'utf8',
  })
  const map = new Map()
  for (const line of out.split('\n')) {
    const [name, hash] = line.trim().split(/\s+/)
    if (name && hash) {
      map.set(name, hash.toLowerCase())
    }
  }
  return map
}

function main() {
  mkdirSync(ENHANCE_DIR, { recursive: true })
  const sums = fetchChecksums(SHERPA_ENHANCE_RELEASE_BASE)
  for (const { file, sha256: pinned } of SHERPA_ENHANCE_MODELS) {
    // checksum.txt of the release lists only gtcrn_simple.onnx; the DPDFNet files carry a pinned
    // hash in the catalog.
    const expected = sums.get(file) ?? pinned
    if (!expected) {
      console.error(
        `${file} is neither listed in ${SHERPA_ENHANCE_RELEASE_BASE}/checksum.txt nor pinned in the catalog`,
      )
      process.exit(1)
    }
    const dest = join(ENHANCE_DIR, file)
    if (existsSync(dest) && sha256(dest) === expected) {
      console.log(`present: ${dest}`)
      continue
    }
    const part = `${dest}.part`
    const url = `${SHERPA_ENHANCE_RELEASE_BASE}/${file}`
    console.log(`Downloading ${url}`)
    execFileSync('curl', ['-fsSL', '--retry', '3', '-o', part, url], { stdio: 'inherit' })
    const actual = sha256(part)
    if (actual !== expected) {
      rmSync(part, { force: true })
      console.error(`checksum mismatch for ${file}: expected ${expected}, got ${actual}`)
      process.exit(1)
    }
    renameSync(part, dest)
  }
  console.log(`\n✓ Denoiser models ready under ${ENHANCE_DIR}`)
}

main()
