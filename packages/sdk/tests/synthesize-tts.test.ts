import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'

import { describe, expect, test } from 'vitest'

import { SynthesizeTtsError, synthesizeTts } from '../src/voice'

describe('synthesizeTts', () => {
  test('mock vendor returns mono 24 kHz pcm', async () => {
    const out = await synthesizeTts({ provider: 'mock' }, 'hello there')
    expect(out.sampleRate).toBe(24000)
    expect(out.channels).toBe(1)
    expect(out.pcm.length).toBeGreaterThan(0)
    expect(out.pcm.length % 2).toBe(0)
  })

  test('empty text rejects with invalid_input', async () => {
    const error = await synthesizeTts({ provider: 'mock' }, '').catch((e: unknown) => e)
    expect(error).toBeInstanceOf(SynthesizeTtsError)
    expect((error as SynthesizeTtsError).code).toBe('invalid_input')
  })

  test('text over 1000 characters rejects with invalid_input', async () => {
    const error = await synthesizeTts({ provider: 'mock' }, 'x'.repeat(1001)).catch(
      (e: unknown) => e,
    )
    expect((error as SynthesizeTtsError).code).toBe('invalid_input')
  })

  test('error message never contains the api key', async () => {
    const apiKey = 'sk-test-SECRET-123'
    // Unreachable endpoint: the vendor call fails fast and the message must not echo the key.
    const error = await synthesizeTts(
      { provider: 'openai', apiKey, endpoint: 'http://127.0.0.1:1' },
      'hello',
      3000,
    ).catch((e: unknown) => e)
    expect(error).toBeInstanceOf(SynthesizeTtsError)
    expect((error as SynthesizeTtsError).message).not.toContain(apiKey)
  })

  // test.mock-hides-runtime / infra.tls-provider-unselected: a fresh process that only loads the
  // bindings and makes an HTTPS vendor call must not hit the rustls "no CryptoProvider" panic.
  test('fresh process: HTTPS vendor call reaches the network layer without a TLS provider panic', () => {
    const bindings = createRequire(import.meta.url).resolve('@node-webrtc-rust/bindings')
    const script = `
      const b = require(${JSON.stringify(bindings)});
      b.synthesizeTts({ provider: b.JsTtsVendor.Deepgram, apiKey: 'k' , endpoint: 'https://127.0.0.1:1' }, 'hello', 3000)
        .then(() => { console.log('ok'); }, (e) => { console.log('rejected:' + e.message); });
    `
    const out = execFileSync(process.execPath, ['-e', script], {
      encoding: 'utf8',
      timeout: 20000,
    })
    expect(out).toContain('rejected:')
    expect(out).not.toContain('CryptoProvider')
  })
})
