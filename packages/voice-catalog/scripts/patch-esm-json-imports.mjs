#!/usr/bin/env node
// tsc CJS (NodeNext) rejects `with { type: "json" }` (TS2856). Patch ESM emit so
// Node 22+ can `import` this package without ERR_IMPORT_ATTRIBUTE_MISSING.
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const esmIndex = join(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  'dist',
  'esm',
  'src',
  'index.js',
)

let source = readFileSync(esmIndex, 'utf8')
const replacements = [
  ["from '../catalog/vendors.json';", "from '../catalog/vendors.json' with { type: 'json' };"],
  ['from "../catalog/vendors.json";', 'from "../catalog/vendors.json" with { type: "json" };'],
  [
    "from '../catalog/azure-tts-voices.json';",
    "from '../catalog/azure-tts-voices.json' with { type: 'json' };",
  ],
  [
    'from "../catalog/azure-tts-voices.json";',
    'from "../catalog/azure-tts-voices.json" with { type: "json" };',
  ],
]

let changed = 0
for (const [from, to] of replacements) {
  if (source.includes(from)) {
    source = source.replaceAll(from, to)
    changed += 1
  }
}

if (changed < 2) {
  throw new Error(`patch-esm-json-imports: expected vendors + azure json imports in ${esmIndex}`)
}

writeFileSync(esmIndex, source)
