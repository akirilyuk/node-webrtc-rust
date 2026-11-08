import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { toJsVadConfig } from '../src/voice/VoiceAgent.js'
import { DEFAULT_VOICE_AGENT_VAD, VOICE_AGENT_VAD_PRESET } from '../src/voice/defaults.js'

const here = dirname(fileURLToPath(import.meta.url))

describe('voice agent VAD end-of-turn defaults', () => {
  it('DEFAULT_VOICE_AGENT_VAD uses minSilenceDurationMs 1100 and sttGateHoldMs 400', () => {
    expect(DEFAULT_VOICE_AGENT_VAD.minSilenceDurationMs).toBe(1100)
    expect(DEFAULT_VOICE_AGENT_VAD.sttGateHoldMs).toBe(400)
  })

  it('VOICE_AGENT_VAD_PRESET inherits the same timings', () => {
    expect(VOICE_AGENT_VAD_PRESET.minSilenceDurationMs).toBe(1100)
    expect(VOICE_AGENT_VAD_PRESET.sttGateHoldMs).toBe(400)
  })

  it('TS defaults equal the Rust binding fallbacks and the core VadConfig defaults', () => {
    const types = readFileSync(resolve(here, '../../bindings/src/speech/types.rs'), 'utf8')
    const config = readFileSync(resolve(here, '../../../crates/speech/src/config.rs'), 'utf8')
    const silenceFallback = /min_silence_duration_ms\.unwrap_or\((\d+)\)/.exec(types)?.[1]
    const holdFallback = /stt_gate_hold_ms\.unwrap_or\((\d+)\)/.exec(types)?.[1]
    const coreSilence = /fn default_min_silence_ms\(\) -> u32 \{\s*(\d+)/.exec(config)?.[1]
    const coreHold = /fn default_stt_gate_hold_ms\(\) -> u32 \{\s*(\d+)/.exec(config)?.[1]
    expect(Number(silenceFallback)).toBe(DEFAULT_VOICE_AGENT_VAD.minSilenceDurationMs)
    expect(Number(holdFallback)).toBe(DEFAULT_VOICE_AGENT_VAD.sttGateHoldMs)
    expect(Number(coreSilence)).toBe(DEFAULT_VOICE_AGENT_VAD.minSilenceDurationMs)
    expect(Number(coreHold)).toBe(DEFAULT_VOICE_AGENT_VAD.sttGateHoldMs)
  })

  it('default provider stays energy and adaptive is not set', () => {
    expect(DEFAULT_VOICE_AGENT_VAD.provider ?? 'energy').toBe('energy')
    expect(VOICE_AGENT_VAD_PRESET.provider ?? 'energy').toBe('energy')
    expect(toJsVadConfig({})?.adaptive).toBeUndefined()
  })

  it('maps provider adaptive and every adaptive field to the native config', () => {
    const adaptive = {
      warmupMs: 500,
      noiseWindowMs: 4000,
      noisePercentile: 60,
      noiseMarginDb: 8,
      useSpeechLevel: false,
      hysteresisDb: 2,
      minThresholdDbfs: -55,
      maxThresholdDbfs: -12,
      stuckActiveMs: 9000,
    }
    const js = toJsVadConfig({ provider: 'adaptive', threshold: 0.2, adaptive })
    expect(js?.provider).toBe('adaptive')
    expect(js?.threshold).toBe(0.2)
    expect(js?.adaptive).toEqual(adaptive)
  })

  it('DEFAULT_VOICE_AGENT_VAD and VOICE_AGENT_VAD_PRESET equal the catalog presets on every vad path', () => {
    const catalog = JSON.parse(
      readFileSync(resolve(here, '../../voice-catalog/catalog/voice-agent-settings.json'), 'utf8'),
    ) as {
      settings: Record<string, { default: unknown }>
      presets: Record<string, Record<string, unknown>>
    }
    const resolveValue = (chain: string[], path: string): unknown => {
      for (const name of chain) {
        if (Object.prototype.hasOwnProperty.call(catalog.presets[name], path)) {
          return catalog.presets[name][path]
        }
      }
      return catalog.settings[path].default
    }
    const flatten = (obj: Record<string, unknown>, prefix: string): Record<string, unknown> =>
      Object.entries(obj).reduce<Record<string, unknown>>((acc, [k, v]) => {
        if (v !== null && typeof v === 'object')
          Object.assign(acc, flatten(v as Record<string, unknown>, `${prefix}${k}.`))
        else acc[`${prefix}${k}`] = v
        return acc
      }, {})
    const cases: Array<[string, Record<string, unknown>, string[]]> = [
      ['DEFAULT_VOICE_AGENT_VAD', DEFAULT_VOICE_AGENT_VAD as Record<string, unknown>, ['default']],
      [
        'VOICE_AGENT_VAD_PRESET',
        VOICE_AGENT_VAD_PRESET as Record<string, unknown>,
        ['voiceAgent', 'default'],
      ],
    ]
    for (const [name, vad, chain] of cases) {
      const flat = flatten(vad, 'vad.')
      expect(Object.keys(flat).length, name).toBeGreaterThan(10)
      for (const [path, value] of Object.entries(flat)) {
        expect(catalog.settings[path], `${name}: ${path} missing from the catalog`).toBeDefined()
        expect(value, `${name}: ${path}`).toBe(resolveValue(chain, path))
      }
    }
  })
})
