import { describe, expect, it } from 'vitest'

import type { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { ListenerUtteranceCollector } from './roundtrip-counting.js'

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

describe('ListenerUtteranceCollector partial fallback', () => {
  it('returns the real final that arrives after the wall-clock partial fallback', async () => {
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()

    let playbackDone!: () => void
    const playback = new Promise<void>((resolve) => {
      playbackDone = resolve
    })
    // 20 ms stands in for the 1850 ms fallback; the final below arrives only after it fired.
    const waited = collector.waitForNextAfterPlayback(playback, 5_000, 20)

    stub.emit({ type: 'user_speech_partial', text: 'Okay, one, two, three, four, five' })
    playbackDone()
    const truncated = await waited
    expect(truncated).toBe('Okay, one, two, three, four, five')

    const settled = collector.finalAfterPartialFallback(truncated, 5_000)
    stub.emit({
      type: 'user_speech_final',
      text: 'Okay, one, two, three, four, five, six, seven, eight, nine, ten',
    })
    await expect(settled).resolves.toBe(
      'Okay, one, two, three, four, five, six, seven, eight, nine, ten',
    )
  })

  it('returns the partial unchanged when no final ever arrives (assertions then fail loudly)', async () => {
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()

    let playbackDone!: () => void
    const playback = new Promise<void>((resolve) => {
      playbackDone = resolve
    })
    const waited = collector.waitForNextAfterPlayback(playback, 5_000, 20)
    stub.emit({ type: 'user_speech_partial', text: 'one two three' })
    playbackDone()
    const truncated = await waited

    await expect(collector.finalAfterPartialFallback(truncated, 50)).resolves.toBe('one two three')
  })

  it('does not wait when the wait resolved from a real final', async () => {
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()

    const waited = collector.waitForNext(5_000, 20)
    stub.emit({ type: 'user_speech_final', text: 'one two three' })
    const text = await waited

    await expect(collector.finalAfterPartialFallback(text, 5_000)).resolves.toBe('one two three')
  })
})
