import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

/** A length cap followed by dropping the head of the queue. */
const DROP_OLDEST_ON_CAP = /if\s*\(\s*[\w.]+\.length\s*>=?\s*[\w.]+\s*\)\s*\{?\s*[\w.]+\.shift\(\)/

/** Rust form: a `len() >=` cap followed by `pop_front()` on the queue. */
const RUST_DROP_OLDEST_ON_CAP =
  /if\s+[\w.]+\.len\(\)\s*>=?\s*[\w.:]+\s*\{\s*(?:let\s+_\s*=\s*)?[\w.]+\.pop_front\(\)/

/** Reviewed exceptions as `<path relative to packages/>:<line>`. */
const ALLOWLIST: string[] = []

function rsFiles(dir: string): string[] {
  const out: string[] = []
  for (const name of readdirSync(dir)) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) out.push(...rsFiles(full))
    else if (full.endsWith('.rs')) out.push(full)
  }
  return out
}

function tsFiles(dir: string): string[] {
  const out: string[] = []
  for (const name of readdirSync(dir)) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) out.push(...tsFiles(full))
    else if (full.endsWith('.ts')) out.push(full)
  }
  return out
}

describe('audio queue drop guard', () => {
  it('audio queues never drop the oldest frame on a length cap (rt.silent-drop-on-full-queue)', () => {
    const packagesDir = resolve(__dirname, '../..')
    const hits: string[] = []
    for (const root of ['helpers/src', 'sdk/src']) {
      for (const file of tsFiles(join(packagesDir, root))) {
        const original = readFileSync(file, 'utf8')
        const stripped = original.replace(/\/\/[^\n]*/g, (m) => ' '.repeat(m.length))
        const re = new RegExp(DROP_OLDEST_ON_CAP.source, 'g')
        for (const match of stripped.matchAll(re)) {
          const line = stripped.slice(0, match.index).split('\n').length
          const key = `${file.slice(packagesDir.length + 1)}:${line}`
          if (!ALLOWLIST.includes(key)) hits.push(key)
        }
      }
    }
    expect(hits, `queue length cap followed by shift(): ${hits.join(', ')}`).toEqual([])
  })

  it('native audio queues never drop the oldest frame on a length cap (rt.silent-drop-on-full-queue)', () => {
    const packagesDir = resolve(__dirname, '../..')
    const hits: string[] = []
    for (const file of rsFiles(join(packagesDir, 'bindings/src'))) {
      const original = readFileSync(file, 'utf8')
      const stripped = original.replace(/\/\/[^\n]*/g, (m) => ' '.repeat(m.length))
      const re = new RegExp(RUST_DROP_OLDEST_ON_CAP.source, 'g')
      for (const match of stripped.matchAll(re)) {
        const line = stripped.slice(0, match.index).split('\n').length
        const key = `${file.slice(packagesDir.length + 1)}:${line}`
        if (!ALLOWLIST.includes(key)) hits.push(key)
      }
    }
    expect(hits, `queue len() cap followed by pop_front(): ${hits.join(', ')}`).toEqual([])
  })

  it('the Rust scan regex matches a drop-oldest snippet', () => {
    const old = 'if frames.len() >= MAX_FRAMES {\n        frames.pop_front();'
    expect(RUST_DROP_OLDEST_ON_CAP.test(old)).toBe(true)
    expect(RUST_DROP_OLDEST_ON_CAP.test('if q.len() > 3 { q.push_back(x); }')).toBe(false)
  })

  it('the scan regex matches the old drop-oldest snippet', () => {
    const old = 'if (state.ttsQueue.length >= TTS_QUEUE_MAX_FRAMES) {\n      state.ttsQueue.shift()'
    expect(DROP_OLDEST_ON_CAP.test(old)).toBe(true)
  })
})
