import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { JsSessionRecorder } from '@node-webrtc-rust/bindings'
import { afterEach, describe, expect, it } from 'vitest'

import {
  pcmFromWriteSampleTeeArgs,
  resolveSessionRecorderOptionsFromEnv,
  SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS,
  SessionRecorder,
} from '../src/session-recorder.js'

/**
 * Quality / pre-push helpers vitest runs without a freshly built `.node`.
 * Published optional bindings (e.g. 0.7.0) may load but omit {@link JsSessionRecorder}
 * until the next bindings release — skip native fidelity until the symbol is a constructor.
 * Integration (`npm test` after compile-native) rebuilds from source and runs these.
 */
const sessionRecorderNativeAvailable = typeof JsSessionRecorder === 'function'

const envBackup = { ...process.env }

afterEach(() => {
  process.env = { ...envBackup }
})

function stereoToneFrame(amplitude: number, frames = 480): Uint8Array {
  const out = new Uint8Array(frames * 4)
  const view = new DataView(out.buffer)
  for (let i = 0; i < frames; i++) {
    view.setInt16(i * 4, amplitude, true)
    view.setInt16(i * 4 + 2, amplitude, true)
  }
  return out
}

function readWavPcm(wav: Buffer): DataView {
  expect(wav.toString('ascii', 0, 4)).toBe('RIFF')
  expect(wav.toString('ascii', 8, 12)).toBe('WAVE')
  return new DataView(wav.buffer, wav.byteOffset + 44, wav.byteLength - 44)
}

function leftPeakAtFrame(view: DataView, frameIndex: number): number {
  return Math.abs(view.getInt16(frameIndex * 4, true))
}

function rightPeakAtFrame(view: DataView, frameIndex: number): number {
  return Math.abs(view.getInt16(frameIndex * 4 + 2, true))
}

describe('resolveSessionRecorderOptionsFromEnv', () => {
  it('defaults format opus and bitrate 256000', () => {
    delete process.env.NWR_RECORD_FORMAT
    delete process.env.NWR_RECORD_BITRATE_BPS
    delete process.env.NWR_RECORD_MAX_SEC
    expect(resolveSessionRecorderOptionsFromEnv()).toEqual({
      format: 'opus',
      bitrateBps: SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS,
      maxDurationMs: 90_000,
    })
  })

  it('parses wav format and custom max sec', () => {
    process.env.NWR_RECORD_FORMAT = 'wav'
    process.env.NWR_RECORD_BITRATE_BPS = '128000'
    process.env.NWR_RECORD_MAX_SEC = '120'
    expect(resolveSessionRecorderOptionsFromEnv()).toEqual({
      format: 'wav',
      bitrateBps: 128_000,
      maxDurationMs: 120_000,
    })
  })
})

describe('pcmFromWriteSampleTeeArgs', () => {
  it('accepts Fatal and CalleeHandled shapes', () => {
    const pcm = Buffer.alloc(4)
    expect(pcmFromWriteSampleTeeArgs([pcm, 20])).toBe(pcm)
    expect(pcmFromWriteSampleTeeArgs([null, pcm, 20])).toBe(pcm)
    expect(pcmFromWriteSampleTeeArgs([null, 20])).toBeNull()
  })
})

function sleepMs(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

describe.skipIf(!sessionRecorderNativeAvailable)('SessionRecorder WAV fidelity', () => {
  it('orders inbound A → outbound B → inbound C on L/R without mid-utterance clipping', async () => {
    const recorder = new SessionRecorder({ format: 'wav', maxDurationMs: 5_000 })
    // Wall gaps mirror e2e serializeSpeechTurnsByWallTime fixture (ready → client → echo).
    recorder.pushInbound(stereoToneFrame(9000, 480))
    await sleepMs(100)
    recorder.pushOutbound(stereoToneFrame(5000, 480))
    await sleepMs(700)
    recorder.pushInbound(stereoToneFrame(8000, 480))

    const dir = mkdtempSync(join(tmpdir(), 'nwr-session-rec-'))
    try {
      const wavPath = join(dir, 'test.wav')
      const result = recorder.finalize(wavPath)
      expect(result.outboundFrames).toBeGreaterThanOrEqual(1)
      expect(result.inboundFrames).toBeGreaterThanOrEqual(2)
      expect(result.durationMs).toBeGreaterThanOrEqual(19)

      const view = readWavPcm(readFileSync(wavPath))
      const totalFrames = Math.floor(view.byteLength / 4)
      // First segment: R ready (L silent).
      expect(leftPeakAtFrame(view, 0)).toBeLessThan(500)
      expect(rightPeakAtFrame(view, 0)).toBeGreaterThan(8000)
      // Client on L after ready + ~100ms wall silence (~480 + 4800 frames at 48kHz).
      // Search the full capture — a hard upper bound of 5000 missed client when sleep ≥100ms (release CI flake).
      let clientFrame = -1
      for (let i = 400; i < totalFrames; i++) {
        if (leftPeakAtFrame(view, i) > 4000) {
          clientFrame = i
          break
        }
      }
      expect(clientFrame).toBeGreaterThan(400)
      expect(rightPeakAtFrame(view, clientFrame)).toBeLessThan(500)
      // Echo on R after client — search remainder of capture.
      let echoFrame = -1
      for (let i = clientFrame + 400; i < totalFrames; i++) {
        if (rightPeakAtFrame(view, i) > 7000) {
          echoFrame = i
          break
        }
      }
      expect(echoFrame).toBeGreaterThan(clientFrame)
      expect(leftPeakAtFrame(view, echoFrame)).toBeLessThan(500)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  it('keeps contiguous burst frames without overwrite', () => {
    const recorder = new SessionRecorder({ format: 'wav', maxDurationMs: 5_000 })
    recorder.pushOutbound(stereoToneFrame(1000, 480))
    recorder.pushOutbound(stereoToneFrame(8000, 480))
    const dir = mkdtempSync(join(tmpdir(), 'nwr-session-rec-'))
    try {
      const wavPath = join(dir, 'burst.wav')
      recorder.finalize(wavPath)
      const view = readWavPcm(readFileSync(wavPath))
      expect(view.getInt16(0, true)).toBe(1000)
      expect(view.getInt16(480 * 4, true)).toBe(8000)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  it('wrapOutboundTrack tees writeSample and setWriteSampleTee', async () => {
    const recorder = new SessionRecorder({ format: 'wav', maxDurationMs: 5_000 })
    let teeCb: ((...args: unknown[]) => void) | null = null
    let jsWriteCount = 0
    const mic = {
      writeSample: async (_data: Uint8Array, _durationMs: number) => {
        jsWriteCount += 1
      },
      setWriteSampleTee: (cb: ((...args: unknown[]) => void) | null) => {
        teeCb = cb
      },
    }
    const wrapped = recorder.wrapOutboundTrack(mic)
    expect(teeCb).toBeTypeOf('function')
    ;(teeCb as (...args: unknown[]) => void)(Buffer.from(stereoToneFrame(4000)), 20)
    await wrapped.writeSample(stereoToneFrame(6000), 20)
    expect(jsWriteCount).toBe(1)
    expect(recorder.isClosed).toBe(false)
    const dir = mkdtempSync(join(tmpdir(), 'nwr-session-rec-'))
    try {
      const result = recorder.finalize(join(dir, 'wrap.wav'))
      expect(result.outboundFrames).toBe(2)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})
