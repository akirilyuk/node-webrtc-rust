/**
 * Clip playback helpers — URL disk cache, progressive fetch, and PCM volume scaling.
 *
 * Native decode/clock lives in `@node-webrtc-rust/sdk/player`; the host must pump
 * {@link takeClipFrame} on a 20 ms cadence.
 */

import { createHash } from 'node:crypto'
import {
  writeFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  unlinkSync,
} from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

import {
  playClip,
  playClipBytes,
  playClipProgressive,
  type ProgressiveClipWriter,
} from '@node-webrtc-rust/sdk/player'

export type AudioPlaySource = { url: string } | { path: string } | { bytes: Buffer | Uint8Array }

export type ClipFetchFn = (url: string) => Promise<Uint8Array>

export type ClipStreamFetchFn = (
  url: string,
  onChunk: (chunk: Uint8Array) => void,
) => Promise<Uint8Array>

export const DEFAULT_CLIP_CACHE_MAX_BYTES = 50 * 1024 * 1024

export type UrlClipCacheOptions = {
  cacheDir?: string
  maxBytes?: number
  fetch?: ClipFetchFn
  /** Progressive URL fetch — append chunks as they arrive. */
  streamFetch?: ClipStreamFetchFn
}

export type ClipPlayerBindings = {
  playClip: (path: string) => string
  playClipBytes: (data: Uint8Array | Buffer) => string
  playClipProgressive: () => { playId: string; writer: ProgressiveClipWriter }
}

const defaultBindings: ClipPlayerBindings = {
  playClip,
  playClipBytes,
  playClipProgressive,
}

/** MixGraph participant id for an auxiliary clip input. */
export function clipPlayInputId(playId: string): string {
  return `play:${playId}`
}

/** Apply linear volume (0–1) to interleaved stereo PCM. */
export function scaleStereoPcmVolume(frame: Buffer, volume: number): Buffer {
  if (volume >= 1) return frame
  if (volume <= 0) return Buffer.alloc(frame.length)
  const out = Buffer.alloc(frame.length)
  for (let i = 0; i + 1 < frame.length; i += 2) {
    const scaled = Math.round(frame.readInt16LE(i) * volume)
    out.writeInt16LE(Math.max(-32_768, Math.min(32_767, scaled)), i)
  }
  return out
}

function hashUrl(url: string): string {
  return createHash('sha256').update(url).digest('hex')
}

function defaultCacheDir(): string {
  return join(tmpdir(), 'node-webrtc-rust-clip-cache')
}

function defaultFetch(url: string): Promise<Uint8Array> {
  return fetch(url).then(async (res) => {
    if (!res.ok) {
      throw new Error(`clip fetch failed (${res.status}): ${url}`)
    }
    return new Uint8Array(await res.arrayBuffer())
  })
}

async function defaultStreamFetch(
  url: string,
  onChunk: (chunk: Uint8Array) => void,
): Promise<Uint8Array> {
  const res = await fetch(url)
  if (!res.ok) {
    throw new Error(`clip fetch failed (${res.status}): ${url}`)
  }
  if (!res.body) {
    const bytes = new Uint8Array(await res.arrayBuffer())
    onChunk(bytes)
    return bytes
  }
  const reader = res.body.getReader()
  const chunks: Uint8Array[] = []
  let total = 0
  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    if (!value) continue
    chunks.push(value)
    total += value.byteLength
    onChunk(value)
  }
  const bytes = new Uint8Array(total)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return bytes
}

/** URL → on-disk encoded clip cache with a byte budget. */
export class UrlClipDiskCache {
  private readonly cacheDir: string
  private readonly maxBytes: number
  private readonly fetchFn: ClipFetchFn
  private readonly streamFetchFn: ClipStreamFetchFn

  constructor(options?: UrlClipCacheOptions) {
    this.cacheDir = options?.cacheDir ?? defaultCacheDir()
    this.maxBytes = options?.maxBytes ?? DEFAULT_CLIP_CACHE_MAX_BYTES
    this.fetchFn = options?.fetch ?? defaultFetch
    this.streamFetchFn = options?.streamFetch ?? defaultStreamFetch
    if (!existsSync(this.cacheDir)) {
      mkdirSync(this.cacheDir, { recursive: true })
    }
  }

  getCacheDir(): string {
    return this.cacheDir
  }

  cachedPathForUrl(url: string): string {
    return join(this.cacheDir, `${hashUrl(url)}.clip`)
  }

  hasCached(url: string): boolean {
    const path = this.cachedPathForUrl(url)
    try {
      return existsSync(path) && statSync(path).size > 0
    } catch {
      return false
    }
  }

  readCached(url: string): Buffer {
    return readFileSync(this.cachedPathForUrl(url))
  }

  private totalCacheBytes(): number {
    let total = 0
    for (const name of readdirSync(this.cacheDir)) {
      try {
        total += statSync(join(this.cacheDir, name)).size
      } catch {
        /* ignore */
      }
    }
    return total
  }

  private evictIfNeeded(incomingBytes: number): void {
    let total = this.totalCacheBytes()
    const entries = readdirSync(this.cacheDir)
      .map((name) => {
        const path = join(this.cacheDir, name)
        try {
          const mtime = statSync(path).mtimeMs
          const size = statSync(path).size
          return { path, mtime, size }
        } catch {
          return null
        }
      })
      .filter((entry): entry is { path: string; mtime: number; size: number } => entry != null)
      .sort((a, b) => a.mtime - b.mtime)

    while (total + incomingBytes > this.maxBytes && entries.length > 0) {
      const oldest = entries.shift()!
      try {
        unlinkSync(oldest.path)
        total -= oldest.size
      } catch {
        /* ignore */
      }
    }
  }

  private writeCached(url: string, data: Uint8Array): string {
    const path = this.cachedPathForUrl(url)
    this.evictIfNeeded(data.byteLength)
    writeFileSync(path, Buffer.from(data))
    return path
  }

  /** Fetch URL bytes (uses disk cache on hit). */
  async loadUrlBytes(url: string): Promise<{ bytes: Buffer; fromCache: boolean }> {
    if (this.hasCached(url)) {
      return { bytes: this.readCached(url), fromCache: true }
    }
    const fetched = await this.fetchFn(url)
    const bytes = Buffer.from(fetched)
    this.writeCached(url, bytes)
    return { bytes, fromCache: false }
  }

  /**
   * Progressive download: playback starts before the fetch completes.
   * Returns native playId and a promise that settles when caching finishes.
   */
  async startProgressiveUrl(
    url: string,
    bindings: ClipPlayerBindings = defaultBindings,
  ): Promise<{ playId: string; cacheDone: Promise<void> }> {
    if (this.hasCached(url)) {
      const playId = bindings.playClip(this.cachedPathForUrl(url))
      return { playId, cacheDone: Promise.resolve() }
    }

    const { playId, writer } = bindings.playClipProgressive()
    const cacheDone = this.streamFetchFn(url, (chunk) => {
      writer.append(chunk)
    })
      .then((fetched) => {
        writer.markEof()
        this.writeCached(url, fetched)
      })
      .catch((error: unknown) => {
        writer.markEof()
        throw error
      })
    return { playId, cacheDone }
  }
}

export async function startClipPlayback(
  source: AudioPlaySource,
  options?: UrlClipCacheOptions & { bindings?: ClipPlayerBindings },
): Promise<string> {
  const bindings = options?.bindings ?? defaultBindings

  if ('path' in source) {
    return bindings.playClip(source.path)
  }

  if ('bytes' in source) {
    const data = Buffer.isBuffer(source.bytes) ? source.bytes : Buffer.from(source.bytes)
    return bindings.playClipBytes(data)
  }

  const cache = new UrlClipDiskCache(options)
  const { playId } = await cache.startProgressiveUrl(source.url, bindings)
  return playId
}
