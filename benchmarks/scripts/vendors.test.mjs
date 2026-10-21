import assert from 'node:assert/strict'
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import { VENDORS } from './vendors.mjs'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const read = (...p) => readFileSync(path.join(root, ...p), 'utf8')

const benchIds = readdirSync(path.join(root, 'crates'))
  .filter(
    (d) =>
      d.startsWith('vendor-') && existsSync(path.join(root, 'crates', d, 'tests/live_bench.rs')),
  )
  .map((d) => d.slice('vendor-'.length))
  .sort()

test('every live_bench.rs has exactly one VENDORS entry and vice versa', () => {
  assert.equal(benchIds.length, 11)
  assert.deepEqual(VENDORS.map((v) => v.id).sort(), benchIds)
  assert.equal(new Set(VENDORS.map((v) => v.id)).size, VENDORS.length)
})

/** Argument text of every `live_env(...)` call in a bench file. */
function liveEnvCalls(text) {
  const calls = []
  for (const m of text.matchAll(/live_env\(/g)) {
    let depth = 1
    let i = m.index + m[0].length
    const start = i
    while (depth > 0 && i < text.length) {
      if (text[i] === '(') depth += 1
      else if (text[i] === ')') depth -= 1
      i += 1
    }
    calls.push(text.slice(start, i - 1))
  }
  return calls
}

function firstArg(call, text) {
  const arg = call.split(',')[0].trim()
  if (arg.startsWith('"')) return arg.slice(1, -1)
  const constant = new RegExp(`const ${arg}: &str = "([^"]+)"`).exec(text)
  assert.ok(constant, `cannot resolve live_env first argument ${arg}`)
  return constant[1]
}

test('vars and envId match the live_env calls of the bench', () => {
  for (const vendor of VENDORS) {
    const text = read('crates', `vendor-${vendor.id}`, 'tests/live_bench.rs')
    const calls = liveEnvCalls(text).filter((c) => !c.startsWith('vendor_id'))
    assert.ok(calls.length > 0, vendor.id)
    const ids = calls.map((c) => firstArg(c, text))
    assert.equal(ids[0], vendor.envId, `${vendor.id}: envId`)
    if (vendor.anyOf) {
      assert.deepEqual(ids.sort(), vendor.anyOf.map((s) => s.envId).sort(), `${vendor.id}: sets`)
    }
    const body = calls.join('\n')
    const crateSrc = sourceText(path.join(root, 'crates', `vendor-${vendor.id}`, 'src'))
    for (const v of vendor.vars) {
      if (v.crateEnv) {
        // Read by the crate itself: optional, and a literal in the crate source.
        assert.ok(v.optional, `${vendor.id}: crateEnv var ${v.name} must be optional`)
        assert.ok(crateSrc.includes(`"${v.name}"`), `${vendor.id}: ${v.name} is not in the crate`)
        assert.equal(body.includes(`"${v.name}"`), false, `${vendor.id}: ${v.name} in live_env`)
        continue
      }
      assert.ok(body.includes(`"${v.name}"`), `${vendor.id}: ${v.name} is not read by live_env`)
    }
    // Every variable of a live_env call is in the table.
    for (const m of body.matchAll(/"([A-Z][A-Z0-9_]+)"/g)) {
      assert.ok(
        vendor.vars.some((v) => v.name === m[1]),
        `${vendor.id}: ${m[1]} missing from vars`,
      )
    }
  }
})

test('every secret is mapped to its variable in vendor-bench.yml', () => {
  const workflow = read('.github/workflows/vendor-bench.yml')
  for (const vendor of VENDORS) {
    for (const v of vendor.vars) {
      if (!v.secret) continue
      assert.ok(
        new RegExp(`^\\s+${v.name}: \\$\\{\\{ secrets\\.${v.secret} \\}\\}\\s*$`, 'm').test(
          workflow,
        ),
        `${vendor.id}: ${v.name} is not mapped to secrets.${v.secret}`,
      )
    }
  }
})

function sourceText(dir) {
  let out = ''
  for (const entry of readdirSync(dir)) {
    const p = path.join(dir, entry)
    if (statSync(p).isDirectory()) out += sourceText(p)
    else if (p.endsWith('.rs')) out += readFileSync(p, 'utf8')
  }
  return out
}

// contract.invented-vendor-api: the probe only touches hosts the crate itself uses.
test('every probed host appears as a literal in the vendor crate source', () => {
  for (const vendor of VENDORS) {
    if (vendor.sdkCrates) {
      const manifest = read('crates', `vendor-${vendor.id}`, 'Cargo.toml')
      for (const sdk of vendor.sdkCrates) {
        assert.ok(manifest.includes(sdk), `${vendor.id}: crate does not depend on ${sdk}`)
      }
      continue
    }
    const src = sourceText(path.join(root, 'crates', `vendor-${vendor.id}`, 'src'))
    for (const host of vendor.hosts) {
      const literal = host.replace('{region}', '')
      assert.ok(
        src.includes(literal),
        `${vendor.id}: host ${host} is not in crates/vendor-${vendor.id}/src`,
      )
    }
  }
})

test('templated hosts name their region variable', () => {
  for (const vendor of VENDORS) {
    if (vendor.hosts.some((h) => h.includes('{region}'))) {
      assert.ok(
        vendor.vars.some((v) => v.name === vendor.regionVar),
        vendor.id,
      )
    }
  }
})
