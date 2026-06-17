#!/usr/bin/env node
/**
 * Post-process tsc ESM output: Node requires `.js` on relative imports.
 * tsc with moduleResolution "Bundler" leaves extensionless paths in dist/esm.
 */
import fs from 'node:fs'
import path from 'node:path'

const root = process.argv[2]
if (!root) {
  console.error('usage: fix-esm-import-extensions.mjs <dist/esm-dir>')
  process.exit(1)
}

const SPECIFIER_RE =
  /((?:import|export)\s+(?:[^'";]*?\s+from\s+|))['"](\.\.?\/[^'"]+)['"]/g

function withJsExtension(specifier) {
  if (/\.(js|json|node|mjs|cjs)$/.test(specifier)) {
    return specifier
  }
  return `${specifier}.js`
}

function fixFile(filePath) {
  const original = fs.readFileSync(filePath, 'utf8')
  const updated = original.replace(
    SPECIFIER_RE,
    (_match, prefix, specifier) => `${prefix}'${withJsExtension(specifier)}'`,
  )
  if (updated !== original) {
    fs.writeFileSync(filePath, updated)
  }
}

function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const fullPath = path.join(dir, entry.name)
    if (entry.isDirectory()) {
      walk(fullPath)
      continue
    }
    if (entry.name.endsWith('.js')) {
      fixFile(fullPath)
    }
  }
}

walk(path.resolve(root))
