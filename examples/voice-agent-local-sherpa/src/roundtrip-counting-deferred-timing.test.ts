import { afterEach, describe, expect, it, vi } from 'vitest'

import type { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { evaluateSpeakingEndFinalTiming, ListenerUtteranceCollector } from './roundtrip-counting.js'

type StubEvent = { type: string; text?: string }

/** Listener whose speech events the test pushes by hand (no timers, no Sherpa). */
function createStubListener() {
  const queue: StubEvent[] = []
  let wake: (() => void) | null = null
  const listener = {
    async *speechEvents() {
      for (;;) {
        while (queue.length === 0) {
          await new Promise<void>((resolve) => {
            wake = resolve
          })
        }
        yield queue.shift()!
      }
    },
  } as unknown as VoiceAgent
  return {
    listener,
    emit(event: StubEvent): void {
      queue.push(event)
      wake?.()
      wake = null
    },
  }
}

/** Let the collector pump drain the stub queue (no wall-clock wait). */
async function drainPump(): Promise<void> {
  for (let i = 0; i < 10; i++) await new Promise<void>((resolve) => setImmediate(resolve))
}

/**
 * Every `Date.now()` call advances the clock 1 ms, so two reads back to back always straddle a tick.
 * That is the CI case in which the old backfill (final read first, then end) reported −1 ms.
 */
function tickingClock(startMs: number): () => void {
  let now = startMs
  vi.spyOn(Date, 'now').mockImplementation(() => {
    now += 1
    return now
  })
  return () => {
    now += 1_000
  }
}

describe('ListenerUtteranceCollector end→final timing while playback is deferred', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('uses the arrival times of a deferred speaking_end and final (end first, positive gap)', async () => {
    const advance = tickingClock(1_000_000)
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()

    let playbackDone!: () => void
    const playback = new Promise<void>((resolve) => {
      playbackDone = resolve
    })
    const waited = collector.waitForNextAfterPlayback(playback, 5_000, 1_000)

    stub.emit({ type: 'user_speech_partial', text: 'one two three' })
    stub.emit({ type: 'user_speaking_end' })
    await drainPump()
    advance() // 1 s between end and final
    stub.emit({ type: 'user_speech_final', text: 'one two three' })
    await drainPump()
    advance()
    playbackDone()
    await expect(waited).resolves.toBe('one two three')

    const endAt = collector.stats.speakingEndAtMs!
    const finalAt = collector.stats.speechFinalAtMs!
    expect(finalAt - endAt).toBeGreaterThanOrEqual(1_000)
    expect(finalAt - endAt).toBeLessThan(1_100)
    const timing = evaluateSpeakingEndFinalTiming({
      stats: collector.stats,
      maxGapMs: 5_000,
      label: 'listener',
    })
    expect(timing.failures).toEqual([])
  })

  it('never reports final before end when the backfill has no event times (partial fallback)', async () => {
    tickingClock(2_000_000)
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()

    let playbackDone!: () => void
    const playback = new Promise<void>((resolve) => {
      playbackDone = resolve
    })
    const waited = collector.waitForNextAfterPlayback(playback, 5_000, 20)

    stub.emit({ type: 'user_speech_partial', text: 'one two three' })
    await drainPump()
    playbackDone()
    await expect(waited).resolves.toBe('one two three')

    const endAt = collector.stats.speakingEndAtMs!
    const finalAt = collector.stats.speechFinalAtMs!
    expect(finalAt - endAt).toBeGreaterThanOrEqual(0)
  })

  it('deferred final with a backfill read in the same tick as end yields gap 0, not −1', async () => {
    tickingClock(3_000_000)
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()

    let playbackDone!: () => void
    const playback = new Promise<void>((resolve) => {
      playbackDone = resolve
    })
    const waited = collector.waitForNextAfterPlayback(playback, 5_000, 1_000)

    // The CI shape: end then final both arrive during post-TTS silence, accepted at playback finish.
    stub.emit({ type: 'user_speaking_end' })
    stub.emit({
      type: 'user_speech_final',
      text: 'one two three four five six seven eight nine ten',
    })
    await drainPump()
    playbackDone()
    await waited

    const timing = evaluateSpeakingEndFinalTiming({
      stats: collector.stats,
      maxGapMs: 500,
      label: 'listener',
    })
    expect(timing.gapMs).not.toBeNull()
    expect(timing.gapMs!).toBeGreaterThanOrEqual(0)
    expect(timing.failures).toEqual([])
  })
})
