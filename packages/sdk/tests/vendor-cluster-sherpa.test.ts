import { JsSttVendor, JsTtsVendor } from '@node-webrtc-rust/bindings'
import { describe, expect, test } from 'vitest'

import {
  STT_VENDOR_VALUES,
  TTS_VENDOR_VALUES,
  sttVendorToJs,
  ttsVendorToJs,
} from '../src/voice/VoiceAgent'

describe('vendor string → NAPI enum', () => {
  test('every non-mock STT vendor maps to a non-Mock Js vendor', () => {
    for (const vendor of STT_VENDOR_VALUES) {
      const mapped = sttVendorToJs(vendor)
      if (vendor === 'mock') {
        expect(mapped).toBe(JsSttVendor.Mock)
      } else {
        expect(mapped).not.toBe(JsSttVendor.Mock)
        expect(mapped).toBeTruthy()
      }
    }
    expect(sttVendorToJs('cluster-sherpa')).toBe(JsSttVendor.ClusterSherpa)
    expect(sttVendorToJs('cluster-sherpa')).toBe('cluster-sherpa')
    expect(sttVendorToJs('local-sherpa')).toBe(JsSttVendor.LocalSherpa)
  })

  test('every non-mock TTS vendor maps to a non-Mock Js vendor', () => {
    for (const vendor of TTS_VENDOR_VALUES) {
      const mapped = ttsVendorToJs(vendor)
      if (vendor === 'mock') {
        expect(mapped).toBe(JsTtsVendor.Mock)
      } else {
        expect(mapped).not.toBe(JsTtsVendor.Mock)
        expect(mapped).toBeTruthy()
      }
    }
    expect(ttsVendorToJs('cluster-sherpa')).toBe(JsTtsVendor.ClusterSherpa)
    expect(ttsVendorToJs('cluster-sherpa')).toBe('cluster-sherpa')
    expect(ttsVendorToJs('local-sherpa')).toBe(JsTtsVendor.LocalSherpa)
  })
})
