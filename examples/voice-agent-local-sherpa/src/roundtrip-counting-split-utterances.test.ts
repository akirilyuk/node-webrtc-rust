import { describe, expect, it } from 'vitest'

import type { VoiceAgent } from '@node-webrtc-rust/sdk/voice'

import { assertFullCountingEcho } from './roundtrip-counting-echo-lid-multi-assert.js'
import { joinUtteranceFinals, ListenerUtteranceCollector } from './roundtrip-counting.js'

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

const COUNTING_TAIL = ', one, two, three, four, five six seven, eight nine ten'

describe('joinUtteranceFinals', () => {
  it('glues a continuation that starts with punctuation onto the previous final', () => {
    expect(joinUtteranceFinals(['Okay', COUNTING_TAIL])).toBe(`Okay${COUNTING_TAIL}`)
  })

  it('separates plain words with one space and drops empty finals', () => {
    expect(joinUtteranceFinals([' Okay ', '', 'one two'])).toBe('Okay one two')
    expect(joinUtteranceFinals([])).toBe('')
  })
})

describe('ListenerUtteranceCollector.finalsUntilComplete', () => {
  // Starved-host shape from CI: the reply "Okay, one ... ten" is split in two utterances, the
  // first final ("Okay") settles the wait, the rest is decoded much later.
  it('waits for the second utterance after the first final settled the wait', async () => {
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()

    let playbackDone!: () => void
    const playback = new Promise<void>((resolve) => {
      playbackDone = resolve
    })
    const waited = collector.waitForNextAfterPlayback(playback, 5_000, 20)
    stub.emit({ type: 'user_speech_final', text: 'Okay' })
    stub.emit({ type: 'user_speech_partial', text: ', one, two' })
    // Let the pump drain both events (microtasks only) before playback is reported done.
    await new Promise<void>((resolve) => setImmediate(resolve))
    playbackDone()
    const first = await waited
    expect(first).toBe('Okay')
    expect(assertFullCountingEcho(first).ok).toBe(false)

    const completed = collector.finalsUntilComplete(
      (joined) => assertFullCountingEcho(joined).ok,
      5_000,
      first,
    )
    stub.emit({ type: 'user_speech_final', text: COUNTING_TAIL })
    await expect(completed).resolves.toBe(`Okay${COUNTING_TAIL}`)
  })

  it('returns immediately when the fallback transcript is already complete', async () => {
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()
    const full = `Okay${COUNTING_TAIL}`
    await expect(
      collector.finalsUntilComplete((joined) => assertFullCountingEcho(joined).ok, 50, full),
    ).resolves.toBe(full)
  })

  it('returns the truncated text at the deadline when the rest never arrives', async () => {
    const stub = createStubListener()
    const collector = new ListenerUtteranceCollector(stub.listener, { value: false }, false)
    collector.startPump()
    await expect(
      collector.finalsUntilComplete((joined) => assertFullCountingEcho(joined).ok, 50, 'Okay'),
    ).resolves.toBe('Okay')
  })
})
