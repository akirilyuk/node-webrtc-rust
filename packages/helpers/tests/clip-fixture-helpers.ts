import { existsSync, readFileSync } from 'node:fs'
import { createServer, type Server } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

export type ClipEncodingFixture = {
  label: string
  ext: string
  freqHz: number
  path: string
}

export const CLIP_FIXTURE_SPECS: ReadonlyArray<{
  file: string
  ext: string
  freqHz: number
}> = [
  { file: 'tone-wav.wav', ext: 'wav', freqHz: 440 },
  { file: 'tone-mp3.mp3', ext: 'mp3', freqHz: 523 },
  { file: 'tone-flac.flac', ext: 'flac', freqHz: 659 },
  { file: 'tone-ogg.ogg', ext: 'ogg', freqHz: 784 },
  { file: 'tone-aac.aac', ext: 'aac', freqHz: 880 },
  { file: 'tone-m4a.m4a', ext: 'm4a', freqHz: 988 },
  { file: 'tone-pcm.pcm', ext: 'pcm', freqHz: 1_100 },
]

export function clipFixtureDir(): string {
  return join(dirname(fileURLToPath(import.meta.url)), 'fixtures', 'clips')
}

export function loadClipFixtures(): ClipEncodingFixture[] {
  const dir = clipFixtureDir()
  const fixtures: ClipEncodingFixture[] = []
  for (const spec of CLIP_FIXTURE_SPECS) {
    const path = join(dir, spec.file)
    if (!existsSync(path)) continue
    fixtures.push({
      label: spec.ext,
      ext: spec.ext,
      freqHz: spec.freqHz,
      path,
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

export type E2eClipPlaybackWavServer = {
  url: string
  close: () => Promise<void>
}

/** Build a 16 kHz mono s16le WAV (2 s default) for clip-playback-smoke URL parity. */
export function buildE2eClipPlaybackWavBytes(options?: {
  durationSec?: number
  sampleRate?: number
  amplitude?: number
  freqHz?: number
}): Buffer {
  const durationSec = options?.durationSec ?? 2
  const sampleRate = options?.sampleRate ?? 16_000
  const amplitude = options?.amplitude ?? 14_000
  const freqHz = options?.freqHz ?? 440
  const sampleCount = Math.floor(durationSec * sampleRate)
  const dataBytes = sampleCount * 2
  const header = Buffer.alloc(44)
  header.write('RIFF', 0)
  header.writeUInt32LE(36 + dataBytes, 4)
  header.write('WAVE', 8)
  header.write('fmt ', 12)
  header.writeUInt32LE(16, 16)
  header.writeUInt16LE(1, 20)
  header.writeUInt16LE(1, 22)
  header.writeUInt32LE(sampleRate, 24)
  header.writeUInt32LE(sampleRate * 2, 28)
  header.writeUInt16LE(2, 32)
  header.writeUInt16LE(16, 34)
  header.write('data', 36)
  header.writeUInt32LE(dataBytes, 40)

  const pcm = Buffer.alloc(dataBytes)
  for (let i = 0; i < sampleCount; i++) {
    const sample =
      freqHz > 0
        ? Math.round(amplitude * Math.sin((2 * Math.PI * freqHz * i) / sampleRate))
        : amplitude
    pcm.writeInt16LE(Math.max(-32_768, Math.min(32_767, sample)), i * 2)
  }
  return Buffer.concat([header, pcm])
}

/** Local HTTP server mirroring staging `/e2e/clip-playback.wav` for helpers integration. */
export function startE2eClipPlaybackWavServer(
  wavBytes: Buffer = buildE2eClipPlaybackWavBytes(),
): Promise<E2eClipPlaybackWavServer> {
  return new Promise((resolve, reject) => {
    const server: Server = createServer((req, res) => {
      if (req.url !== '/e2e/clip-playback.wav') {
        res.statusCode = 404
        res.end()
        return
      }
      res.writeHead(200, {
        'Content-Type': 'audio/wav',
        'Content-Length': wavBytes.length,
      })
      res.end(wavBytes)
    })
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address()
      if (!addr || typeof addr === 'string') {
        reject(new Error('e2e clip playback server bind failed'))
        return
      }
      resolve({
        url: `http://127.0.0.1:${addr.port}/e2e/clip-playback.wav`,
        close: () =>
          new Promise<void>((done) => {
            server.close(() => done())
          }),
      })
    })
    server.on('error', reject)
  })
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
