import { LocalAudioTrack } from '@node-webrtc-rust/sdk'
import { AudioMixGraph } from '@node-webrtc-rust/sdk/mix'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { ClientAudioMixer } from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'

function nativeAvailable(): boolean {
  try {
    const graph = new AudioMixGraph()
    graph.addInput('probe')
    graph.removeInput('probe')
    return typeof graph.startNativePump === 'function'
  } catch {
    return false
  }
}

/** One stereo frame whose every sample is `value`. */
function constantFrame(value: number): Buffer {
  const frame = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  for (let i = 0; i < frame.length; i += 2) frame.writeInt16LE(value, i)
  return frame
}

function peak(frame: Buffer): number {
  let max = 0
  for (let i = 0; i + 1 < frame.length; i += 2) max = Math.max(max, Math.abs(frame.readInt16LE(i)))
  return max
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function until(cond: () => boolean, timeoutMs: number): Promise<boolean> {
  const end = Date.now() + timeoutMs
  while (Date.now() < end) {
    if (cond()) return true
    await sleep(10)
  }
  return cond()
}

describe.skipIf(!nativeAvailable())('ClientAudioMixer native pump', () => {
  let graph: AudioMixGraph
  let mixer: ClientAudioMixer
  let sidecar: LocalAudioTrack
  let pcOut: LocalAudioTrack
  let captured: Buffer[]

  beforeEach(() => {
    graph = new AudioMixGraph()
    mixer = new ClientAudioMixer({ graph })
    mixer.registerPeer('p1')
    sidecar = mixer.createTtsSidecar('p1')
    pcOut = new LocalAudioTrack('pc-out', 'native-pump-test')
    captured = []
    pcOut.setWriteSampleTee((data) => {
      captured.push(Buffer.from(data))
    })
  })

  afterEach(() => {
    mixer.unregisterPeer('p1')
    pcOut.setWriteSampleTee(null)
    delete process.env.NWR_MIX_NATIVE_PUMP
  })

  async function startNative(): Promise<void> {
    mixer.startMixPump('p1', pcOut)
    expect(await until(() => graph.nativePumpStats('p1') !== null, 5_000)).toBe(true)
  }

  const nonSilent = (): Buffer[] => captured.filter((f) => peak(f) > 0)

  it('native pump keeps every TTS frame when the JS event loop is blocked', async () => {
    await startNative()
    for (let i = 0; i < 150; i++) await sidecar.writeSample(constantFrame(1_000 + 100 * i), 20)
    const end = Date.now() + 1_500
    while (Date.now() < end) {
      // Block the JS thread; the native task must keep writing.
    }
    expect(await until(() => nonSilent().length >= 150, 10_000)).toBe(true)
    await sleep(100)
    const peaks = nonSilent().map(peak)
    expect(peaks).toHaveLength(150)
    for (let i = 1; i < peaks.length; i++) {
      expect(peaks[i]).toBeGreaterThan(peaks[i - 1]!)
    }
  })

  it('native pump writes ~50 frames/s of silence when idle', async () => {
    await startNative()
    const before = captured.length
    await sleep(1_000)
    const frames = captured.length - before
    expect(frames).toBeGreaterThanOrEqual(40)
    expect(frames).toBeLessThanOrEqual(60)
    expect(nonSilent()).toHaveLength(0)
  })

  it('NWR_MIX_NATIVE_PUMP=0 uses the JS pump', async () => {
    process.env.NWR_MIX_NATIVE_PUMP = '0'
    let jsWrites = 0
    const orig = pcOut.writeSample.bind(pcOut)
    pcOut.writeSample = async (data, durationMs) => {
      jsWrites += 1
      return orig(data, durationMs)
    }
    mixer.startMixPump('p1', pcOut)
    expect(await until(() => jsWrites >= 5, 3_000)).toBe(true)
    expect(graph.nativePumpStats('p1')).toBeNull()
    mixer.stopMixPump('p1')
  })

  it('discardQueuedTts in native mode drops queued frames and nothing plays afterwards', async () => {
    await startNative()
    for (let i = 0; i < 100; i++) await sidecar.writeSample(constantFrame(2_000), 20)
    const discarded = mixer.discardQueuedTts('p1')
    expect(discarded).toBeGreaterThanOrEqual(90)
    await sleep(150)
    const playedAfterDiscard = nonSilent().length
    expect(playedAfterDiscard).toBeLessThanOrEqual(100 - discarded + 1)
    await sleep(400)
    expect(nonSilent()).toHaveLength(playedAfterDiscard)
  })
})
