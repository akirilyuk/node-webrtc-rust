#!/usr/bin/env node
/**
 * Ensure the loaded native .node implements every JsPeerConnection method
 * declared in packages/bindings/index.d.ts (committed NAPI contract).
 *
 * Exits 0 when the cache artifact matches the declared surface; exits 1 when
 * the .node is stale (e.g. partial cache restore or skip-build bug).
 */
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '../..')
const bindingsDir = join(root, 'packages/bindings')
const dtsPath = join(bindingsDir, 'index.d.ts')

const dts = readFileSync(dtsPath, 'utf8')
const classBlock = dts.match(/export declare class JsPeerConnection \{([\s\S]*?)\n\}/)?.[1]
if (!classBlock) {
  console.error('JsPeerConnection block not found in index.d.ts')
  process.exit(1)
}

/** Instance methods only — skip getters/setters and constructor. */
const methods = [
  ...classBlock.matchAll(/^\s+(?!get |set |constructor)(\w+)\(/gm),
  ...classBlock.matchAll(/^\s+(setOn\w+)\(/gm),
].map((m) => m[1])

const unique = [...new Set(methods)]
if (unique.length === 0) {
  console.error('No JsPeerConnection methods parsed from index.d.ts')
  process.exit(1)
}

const require = createRequire(join(bindingsDir, 'index.js'))
const bindings = require(join(bindingsDir, 'index.js'))

let pc
try {
  pc = new bindings.JsPeerConnection({})
} catch (error) {
  console.error('Failed to construct JsPeerConnection:', error)
  process.exit(1)
}

const missing = unique.filter((name) => typeof pc[name] !== 'function')
if (missing.length > 0) {
  console.error('Native .node missing NAPI methods declared in index.d.ts:')
  for (const name of missing) {
    console.error(`  - ${name}`)
  }
  process.exit(1)
}

console.log(`Native binding OK — ${unique.length} JsPeerConnection methods verified`)
