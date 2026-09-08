import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { createServer, type Server } from 'node:http'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

const SAMPLE_RATE = 48_000
const FFMPEG = '/opt/homebrew/bin/ffmpeg'

export type ClipEncodingFixture = {
  label: string
  ext: string
  freqHz: number
  path: string
}

export function clipFixtureDir(): string {
  const dir = join(tmpdir(), 'nwr-clip-integration-fixtures')
  mkdirSync(dir, { recursive: true })
  return dir
}

function writeStereoWav(path: string, freqHz: number, durationSec: number, amplitude = 0.25): void {
  const channels = 2
  const numSamples = Math.floor(SAMPLE_RATE * durationSec)
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
    const sample = Math.round(amplitude * 32_767 * Math.sin(2 * Math.PI * freqHz * t))
    const clamped = Math.max(-32_768, Math.min(32_767, sample))
    pcm.writeInt16LE(clamped, i * 4)
    pcm.writeInt16LE(clamped, i * 4 + 2)
  }
  writeFileSync(path, Buffer.concat([header, pcm]))
}

function ffmpegAvailable(): boolean {
  return existsSync(FFMPEG)
}

export function canEncodeClip(ext: string): boolean {
  if (ext === 'wav') return true
  if (!ffmpegAvailable()) return false
  try {
    const dir = clipFixtureDir()
    const src = join(dir, `_probe-${ext}.wav`)
    const out = join(dir, `_probe-${ext}.${ext}`)
    writeStereoWav(src, 440, 0.2)
    const args = encodeArgs(ext, src, out)
    execFileSync(FFMPEG, args, { stdio: 'pipe' })
    return existsSync(out)
  } catch {
    return false
  }
}

function encodeArgs(ext: string, src: string, out: string): string[] {
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

export function generateClipFixtures(): ClipEncodingFixture[] {
  const dir = clipFixtureDir()
  const specs: Array<{ ext: string; freqHz: number }> = [
    { ext: 'wav', freqHz: 440 },
    { ext: 'mp3', freqHz: 523 },
    { ext: 'flac', freqHz: 659 },
    { ext: 'ogg', freqHz: 784 },
    { ext: 'aac', freqHz: 880 },
    { ext: 'm4a', freqHz: 988 },
    { ext: 'pcm', freqHz: 1_100 },
  ]

  const fixtures: ClipEncodingFixture[] = []
  for (const spec of specs) {
    if (!canEncodeClip(spec.ext)) continue
    const wavSrc = join(dir, `tone-${spec.ext}.src.wav`)
    const outPath = join(dir, `tone-${spec.ext}.${spec.ext === 'm4a' ? 'm4a' : spec.ext}`)
    writeStereoWav(wavSrc, spec.freqHz, 1.75)
    if (spec.ext === 'wav') {
      writeFileSync(outPath, readFileSync(wavSrc))
    } else {
      execFileSync(FFMPEG, encodeArgs(spec.ext, wavSrc, outPath), { stdio: 'pipe' })
    }
    fixtures.push({
      label: spec.ext,
      ext: spec.ext,
      freqHz: spec.freqHz,
      path: outPath,
    })
  }
  return fixtures
}

export type HoldbackClipServer = {
  url: string
  releaseTail: () => void
  tailReleased: () => boolean
  close: () => Promise<void>
}

/** HTTP server that streams a WAV with a hold on the tail for progressive-start tests. */
export function startHoldbackWavServer(wavPath: string): Promise<HoldbackClipServer> {
  const fileBytes: Buffer = readFileSync(wavPath)
  // Hold back most of the file so decode must start from an early prefix only.
  const holdAt = Math.min(64 * 1024, Math.max(512, Math.floor(fileBytes.length * 0.2)))
  let tailReleased = false
  let releaseTail!: () => void
  const tailGate = new Promise<void>((resolve) => {
    releaseTail = () => {
      tailReleased = true
      resolve()
    }
  })

  return new Promise((resolve, reject) => {
    const server: Server = createServer((req, res) => {
      if (req.url !== '/tone.wav') {
        res.statusCode = 404
        res.end()
        return
      }
      res.writeHead(200, {
        'Content-Type': 'audio/wav',
        'Transfer-Encoding': 'chunked',
      })
      const head = fileBytes.subarray(0, holdAt)
      res.write(head)
      void tailGate.then(() => {
        const tail = fileBytes.subarray(holdAt)
        if (tail.length > 0) {
          res.write(tail)
        }
        res.end()
      })
    })
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address()
      if (!addr || typeof addr === 'string') {
        reject(new Error('holdback server bind failed'))
        return
      }
      resolve({
        url: `http://127.0.0.1:${addr.port}/tone.wav`,
        releaseTail,
        tailReleased: () => tailReleased,
        close: () =>
          new Promise<void>((done) => {
            server.close(() => done())
          }),
      })
    })
    server.on('error', reject)
  })
}
