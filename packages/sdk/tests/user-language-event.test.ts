import { describe, expect, test } from 'vitest'

import { SPEECH_EVENT_TYPE } from '../src/voice/types'
import { speechEventToControlMessage } from '../src/voice/speech-event-bridge'

describe('user_language speech events', () => {
  test('SPEECH_EVENT_TYPE includes user_language', () => {
    expect(SPEECH_EVENT_TYPE.userLanguage).toBe('user_language')
  })

  test('speechEventToControlMessage forwards language field', () => {
    expect(
      speechEventToControlMessage({
        type: 'user_language',
        language: 'de',
        text: 'de',
      }),
    ).toEqual({
      type: 'speech_event',
      event: 'user_language',
      text: 'de',
      language: 'de',
      ts: undefined,
    })
  })
})
