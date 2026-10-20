import assert from 'node:assert/strict'
import test from 'node:test'

import { parseEnvFile, resolveVendorEnv } from './env-file.mjs'

test('parses whitespace-separated tokens, splitting on the first =', () => {
  const map = parseEnvFile('A=1 B=x=y\nC=z\tD=')
  assert.equal(map.get('A'), '1')
  assert.equal(map.get('B'), 'x=y')
  assert.equal(map.get('C'), 'z')
  assert.equal(map.get('D'), '')
})

test('a bare NAME is an empty string and a token with a bad name is ignored', () => {
  const map = parseEnvFile('BARE FOO#=1 FOO# 9X=2 OK_1=3')
  assert.equal(map.get('BARE'), '')
  assert.equal(map.has('FOO#'), false)
  assert.equal(map.has('9X'), false)
  assert.equal(map.get('OK_1'), '3')
})

const vendor = {
  vars: [
    { name: 'A_KEY', secret: 'A_LIVE_KEY' },
    { name: 'A_REGION', secret: 'A_LIVE_REGION' },
    { name: 'A_OPT', optional: true },
  ],
}

test('the library name wins over the secret name; a secret name is the fallback', () => {
  const file = parseEnvFile('A_KEY=lib A_LIVE_KEY=secret A_LIVE_REGION=eu')
  const { env, missing } = resolveVendorEnv(vendor, [() => undefined, (n) => file.get(n)])
  assert.deepEqual(env, { A_KEY: 'lib', A_REGION: 'eu' })
  assert.deepEqual(missing, [])
})

test('process env is looked up before the env file, and empty values count as unset', () => {
  const file = parseEnvFile('A_KEY=fromfile A_REGION=eu')
  const proc = { A_KEY: 'fromproc', A_REGION: '' }
  const { env } = resolveVendorEnv(vendor, [(n) => proc[n], (n) => file.get(n)])
  assert.equal(env.A_KEY, 'fromproc')
  assert.equal(env.A_REGION, 'eu')
})

test('lists the non-optional variables that are still empty', () => {
  const { env, missing } = resolveVendorEnv(vendor, [(n) => ({ A_OPT: 'x' })[n]])
  assert.deepEqual(env, { A_OPT: 'x' })
  assert.deepEqual(missing, ['A_KEY', 'A_REGION'])
})

test('anyOf: the vendor is runnable when one set is complete', () => {
  const google = {
    vars: [
      { name: 'K', optional: true },
      { name: 'C', optional: true },
      { name: 'P', optional: true },
    ],
    anyOf: [
      { envId: 'g', vars: ['K'] },
      { envId: 'g_sa', vars: ['C', 'P'] },
    ],
  }
  const one = resolveVendorEnv(google, [(n) => ({ K: 'k', C: 'c' })[n]])
  assert.deepEqual(one.sets, ['g'])
  assert.deepEqual(one.missing, [])
  const none = resolveVendorEnv(google, [(n) => ({ C: 'c' })[n]])
  assert.deepEqual(none.sets, [])
  assert.deepEqual(none.missing, ['(K)', '(C + P)'])
})
