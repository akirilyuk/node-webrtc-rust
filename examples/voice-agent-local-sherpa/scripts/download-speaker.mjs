#!/usr/bin/env node
/**
 * Download the assets of the speaker-ID bench into `.models/speaker/` (test and bench only):
 *
 *   npm run download-speaker --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 *   - the kept speaker-embedding ONNX models and the real-voice test wavs, from release
 *     `speaker-recongition-models` (sic), each checked against that release's checksum.txt;
 *   - the Piper bundles of the background voices, from release `tts-models`, extracted next to
 *     the other TTS bundles in `.models/` (checked against the `tts-models` checksum.txt).
 *
 * A file that is already present with a matching hash, or an extracted bundle that has
 * tokens.txt, is skipped. A checksum mismatch deletes nothing and exits 1 naming the file.
 * The main voice (vits-piper-en_US-amy-low) comes from `npm run download-tts:en`.
 */

import { createHash } from 'crypto'
import { execFileSync } from 'child_process'
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync } from 'fs'
import { dirname, join } from 'path'
import { fileURLToPath } from 'url'

import {
  SHERPA_SPEAKER_MODELS,
  SHERPA_SPEAKER_PIPER_BUNDLES,
  SHERPA_SPEAKER_RELEASE_BASE,
  SHERPA_SPEAKER_WAVS,
} from './sherpa-model-catalog.mjs'
import { SHERPA_TTS_RELEASE_BASE } from './sherpa-tts-model-catalog.mjs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const MODELS_DIR = join(__dirname, '..', '.models')
const SPEAKER_DIR = join(MODELS_DIR, 'speaker')

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

function curl(url, dest) {
  console.log(`Downloading ${url}`)
  execFileSync('curl', ['-fsSL', '--retry', '3', '-o', dest, url], { stdio: 'inherit' })
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

function fail(name, expected, actual) {
  console.error(`checksum mismatch for ${name}: expected ${expected}, got ${actual}`)
  process.exit(1)
}

/** Download `base/name` into `dir` unless a file with the expected hash is already there. */
function ensureFile(base, name, dir, checksums) {
  const expected = checksums.get(name)
  if (!expected) {
    console.error(`${name} is not listed in ${base}/checksum.txt`)
    process.exit(1)
  }
  const dest = join(dir, name)
  if (existsSync(dest) && sha256(dest) === expected) {
    console.log(`present: ${dest}`)
    return dest
  }
  const part = `${dest}.part`
  curl(`${base}/${name}`, part)
  const actual = sha256(part)
  if (actual !== expected) {
    rmSync(part, { force: true })
    fail(name, expected, actual)
  }
  renameSync(part, dest)
  return dest
}

function main() {
  mkdirSync(SPEAKER_DIR, { recursive: true })

  const speakerSums = fetchChecksums(SHERPA_SPEAKER_RELEASE_BASE)
  for (const model of SHERPA_SPEAKER_MODELS) {
    ensureFile(SHERPA_SPEAKER_RELEASE_BASE, model.file, SPEAKER_DIR, speakerSums)
  }
  for (const wav of SHERPA_SPEAKER_WAVS) {
    ensureFile(SHERPA_SPEAKER_RELEASE_BASE, wav, SPEAKER_DIR, speakerSums)
  }

  const ttsSums = fetchChecksums(SHERPA_TTS_RELEASE_BASE)
  for (const bundle of SHERPA_SPEAKER_PIPER_BUNDLES) {
    const target = join(MODELS_DIR, bundle)
    if (existsSync(join(target, 'tokens.txt'))) {
      console.log(`present: ${target}`)
      continue
    }
    const archive = ensureFile(SHERPA_TTS_RELEASE_BASE, `${bundle}.tar.bz2`, MODELS_DIR, ttsSums)
    console.log(`Extracting ${archive} …`)
    execFileSync('tar', ['-xjf', archive, '-C', MODELS_DIR], { stdio: 'inherit' })
    if (!existsSync(join(target, 'tokens.txt'))) {
      console.error(`Expected ${target}/tokens.txt after extracting ${archive}`)
      process.exit(1)
    }
  }

  console.log(`\n✓ Speaker-ID bench assets ready under ${MODELS_DIR}`)
  console.log(`  embedding models + wavs: ${SPEAKER_DIR}`)
}

main()
