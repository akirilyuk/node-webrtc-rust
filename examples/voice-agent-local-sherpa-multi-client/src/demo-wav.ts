/**
 * Build a short 48 kHz stereo PCM WAV for clip playback demos/tests.
 */

import { mkdirSync, writeFileSync, existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const SAMPLE_RATE = 48_000
const CHANNELS = 2
const DURATION_MS = 400

export function buildDemoWavBytes(): Buffer {
  const numSamples = (SAMPLE_RATE * DURATION_MS) / 1000
  const dataSize = numSamples * CHANNELS * 2
  const wav = Buffer.alloc(44 + dataSize)
  wav.write('RIFF', 0)
  wav.writeUInt32LE(36 + dataSize, 4)
  wav.write('WAVEfmt ', 8)
  wav.writeUInt32LE(16, 16)
  wav.writeUInt16LE(1, 20)
  wav.writeUInt16LE(CHANNELS, 22)
  wav.writeUInt32LE(SAMPLE_RATE, 24)
  wav.writeUInt32LE(SAMPLE_RATE * CHANNELS * 2, 28)
  wav.writeUInt16LE(CHANNELS * 2, 32)
  wav.writeUInt16LE(16, 34)
  wav.write('data', 36)
  wav.writeUInt32LE(dataSize, 40)
  for (let i = 0; i < numSamples; i++) {
    const t = i / SAMPLE_RATE
    const sample = Math.sin(2 * Math.PI * 660 * t) * 0.15 * 32_767
    const v = Math.max(-32_768, Math.min(32_767, Math.round(sample)))
    for (let ch = 0; ch < CHANNELS; ch++) {
      wav.writeInt16LE(v, 44 + (i * CHANNELS + ch) * 2)
    }
  }
  return wav
}

/** Writes `assets/demo.wav` beside this module when missing. */
export function ensureDemoWavPath(baseDir: string): string {
  const assetsDir = join(baseDir, 'assets')
  const wavPath = join(assetsDir, 'demo.wav')
  if (existsSync(wavPath)) return wavPath
  mkdirSync(assetsDir, { recursive: true })
  writeFileSync(wavPath, buildDemoWavBytes())
  return wavPath
}

export function demoWavPathFromImportMeta(importMetaUrl: string): string {
  const moduleDir = dirname(fileURLToPath(importMetaUrl))
  return ensureDemoWavPath(join(moduleDir, '..'))
}
