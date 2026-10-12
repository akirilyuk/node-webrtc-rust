#!/usr/bin/env node
// Fails when the loaded native binding lacks a function, class, or class member
// declared in index.d.ts (a stale .node). Not part of run-pr-quality.sh: the Release
// quality job intentionally runs with the previously published binding.
//
// Usage: node scripts/ci/check-native-exports.mjs [index.d.ts] [loader index.js]
// Env: NATIVE_EXPORTS_DTS, NATIVE_EXPORTS_LOADER (args win over env).
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..')
const dtsPath = path.resolve(
  process.argv[2] ??
    process.env.NATIVE_EXPORTS_DTS ??
    path.join(root, 'packages/bindings/index.d.ts'),
)
const loaderPath = path.resolve(
  process.argv[3] ??
    process.env.NATIVE_EXPORTS_LOADER ??
    path.join(root, 'packages/bindings/index.js'),
)

/** @returns {{functions: string[], classes: {name: string, methods: string[], statics: string[], props: string[]}[]}} */
export function parseDts(text) {
  const functions = []
  const classes = []
  const lines = text.split(/\r?\n/)
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]
    const fn = /^export declare function (\w+)\s*[(<]/.exec(line)
    if (fn) {
      functions.push(fn[1])
      continue
    }
    const cls = /^export declare class (\w+)\b[^{]*\{\s*$/.exec(line)
    if (!cls) continue
    const entry = { name: cls[1], methods: [], statics: [], props: [] }
    for (i++; i < lines.length && !/^\}/.test(lines[i]); i++) {
      const m = lines[i]
      // Members sit at exactly two-space indent; doc comment lines start with `/**` or ` *`.
      let r = /^ {2}(?:static\s+)(?:async\s+)?(\w+)\s*[(<]/.exec(m)
      if (r) {
        entry.statics.push(r[1])
        continue
      }
      r = /^ {2}(?:get|set)\s+(\w+)\s*\(/.exec(m)
      if (r) {
        if (!entry.props.includes(r[1])) entry.props.push(r[1])
        continue
      }
      r = /^ {2}(?:async\s+)?(\w+)\s*[(<]/.exec(m)
      if (r && r[1] !== 'constructor') entry.methods.push(r[1])
    }
    classes.push(entry)
  }
  return { functions, classes }
}

function main() {
  const { functions, classes } = parseDts(readFileSync(dtsPath, 'utf8'))
  const require = createRequire(loaderPath)
  const binding = require(loaderPath)
  const missing = []
  for (const fn of functions) {
    if (typeof binding[fn] !== 'function') missing.push(fn)
  }
  for (const c of classes) {
    const ctor = binding[c.name]
    if (typeof ctor !== 'function') {
      missing.push(c.name)
      continue
    }
    for (const m of c.methods) if (!(m in ctor.prototype)) missing.push(`${c.name}.${m}`)
    for (const p of c.props) if (!(p in ctor.prototype)) missing.push(`${c.name}.${p}`)
    for (const s of c.statics) if (!(s in ctor)) missing.push(`${c.name}.${s}`)
  }
  if (missing.length > 0) {
    let loaded = loaderPath
    try {
      const hit = Object.keys(require.cache).find((k) => k.endsWith('.node'))
      if (hit) loaded = hit
    } catch {
      // keep loader path
    }
    console.error(
      `native binding is stale: missing ${missing.join(', ')} — run npm run build:native`,
    )
    console.error(`loaded: ${loaded}`)
    process.exit(1)
  }
  console.log(
    `OK: native exports match index.d.ts (${functions.length} functions, ${classes.length} classes)`,
  )
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
