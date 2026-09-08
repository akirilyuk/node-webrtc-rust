#!/usr/bin/env node
/**
 * One-off regeneration of committed clip fixtures (not run by tests/CI).
 * See README.md in this directory.
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const SAMPLE_RATE = 48_000
const DURATION_SEC = 1.75
const AMPLITUDE = 0.25
const FFMPEG = process.env.FFMPEG ?? '/opt/homebrew/bin/ffmpeg'

const SPECS = [
  { file: 'tone-wav.wav', ext: 'wav', freqHz: 440 },
  { file: 'tone-mp3.mp3', ext: 'mp3', freqHz: 523 },
  { file: 'tone-flac.flac', ext: 'flac', freqHz: 659 },
  { file: 'tone-ogg.ogg', ext: 'ogg', freqHz: 784 },
  { file: 'tone-aac.aac', ext: 'aac', freqHz: 880 },
  { file: 'tone-m4a.m4a', ext: 'm4a', freqHz: 988 },
  { file: 'tone-pcm.pcm', ext: 'pcm', freqHz: 1_100 },
]

function writeStereoWav(path, freqHz) {
  const channels = 2
  const numSamples = Math.floor(SAMPLE_RATE * DURATION_SEC)
  const dataSize = numSamples * channels * 2
  const header = Buffer.alloc(44)
  header.write('RIFF', 0)
  header.writeUInt32LE(36 + dataSize, 4)
  header.write('WAVEfmt ', 8)
  header.writeUInt32LE(16, 16)
  header.writeUInt16LE(1, 20)
  header.writeUInt16LE(channels, 22)
  header.writeUInt32LE(SAMPLE_RATE, 24)
  header.writeUInt32LE(SAMPLE_RATE * channels * 2, 28)
  header.writeUInt16LE(channels * 2, 32)
  header.writeUInt16LE(16, 34)
  header.write('data', 36)
  header.writeUInt32LE(dataSize, 40)

  const pcm = Buffer.alloc(dataSize)
  for (let i = 0; i < numSamples; i++) {
    const t = i / SAMPLE_RATE
    const sample = Math.round(AMPLITUDE * 32_767 * Math.sin(2 * Math.PI * freqHz * t))
    const clamped = Math.max(-32_768, Math.min(32_767, sample))
    pcm.writeInt16LE(clamped, i * 4)
    pcm.writeInt16LE(clamped, i * 4 + 2)
  }
  writeFileSync(path, Buffer.concat([header, pcm]))
}

function encodeArgs(ext, src, out) {
  const base = ['-y', '-hide_banner', '-loglevel', 'error', '-i', src]
  switch (ext) {
    case 'mp3':
      return [...base, '-codec:a', 'libmp3lame', '-q:a', '4', out]
    case 'flac':
      return [...base, '-codec:a', 'flac', out]
    case 'ogg':
      return [...base, '-codec:a', 'libvorbis', '-q:a', '4', out]
    case 'aac':
      return [...base, '-codec:a', 'aac', '-f', 'adts', out]
    case 'm4a':
      return [...base, '-codec:a', 'aac', '-movflags', '+faststart', out]
    case 'pcm':
      return [...base, '-f', 's16le', '-ac', '2', '-ar', '48000', out]
    default:
      throw new Error(`unsupported encode ext: ${ext}`)
  }
}

mkdirSync(__dirname, { recursive: true })

if (!existsSync(FFMPEG)) {
  console.error(`ffmpeg not found at ${FFMPEG}`)
  process.exit(1)
}

for (const spec of SPECS) {
  const outPath = join(__dirname, spec.file)
  const wavSrc = join(__dirname, `${spec.file}.src.wav`)
  writeStereoWav(wavSrc, spec.freqHz)
  if (spec.ext === 'wav') {
    writeFileSync(outPath, readFileSync(wavSrc))
  } else {
    execFileSync(FFMPEG, encodeArgs(spec.ext, wavSrc, outPath), { stdio: 'inherit' })
  }
  unlinkSync(wavSrc)
  console.log(`wrote ${spec.file}`)
}
