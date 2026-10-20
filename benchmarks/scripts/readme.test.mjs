// Guards benchmarks/README.md: every results folder it links exists, and the Deepgram and
// AssemblyAI names appear only in the "not published" paragraph, never next to a number.

import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const benchDir = path.resolve(here, '..')
const readme = readFileSync(path.join(benchDir, 'README.md'), 'utf8')

test('every results folder linked from the README exists', () => {
  const dirs = [...readme.matchAll(/\]\((results\/[^)#\s]+)\)/g)].map((m) => m[1])
  assert.ok(dirs.length >= 2, 'README links the dated result folders')
  for (const dir of dirs) {
    assert.ok(existsSync(path.join(benchDir, dir)), `${dir} exists`)
  }
})

test('the README has no Deepgram or AssemblyAI number', () => {
  const marker = '### Not published'
  const at = readme.indexOf(marker)
  assert.notEqual(at, -1, 'README has a Not published section')
  const outside = readme.slice(0, at)
  assert.doesNotMatch(outside, /deepgram|assemblyai/i)
  const notPublished = readme.slice(at).split('\n').slice(0, 6).join('\n')
  assert.doesNotMatch(notPublished, /\d+\s*\/\s*\d+/, 'no median / p90 pair in the section')
})
