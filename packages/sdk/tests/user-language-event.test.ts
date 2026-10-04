import { describe, expect, test } from 'vitest'

import { SPEECH_EVENT_TYPE, type LanguageIdSkipReason } from '../src/voice/types'
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

describe('language_id_skipped speech events', () => {
  test('SPEECH_EVENT_TYPE includes language_id_skipped', () => {
    expect(SPEECH_EVENT_TYPE.languageIdSkipped).toBe('language_id_skipped')
  })

  test('speechEventToControlMessage forwards reason and speechMs without text', () => {
    expect(
      speechEventToControlMessage({
        type: 'language_id_skipped',
        reason: 'too_short',
        speechMs: 120,
        utteranceId: 'utt-1',
      }),
    ).toEqual({
      type: 'speech_event',
      event: 'language_id_skipped',
      reason: 'too_short',
      speechMs: 120,
      ts: undefined,
    })
  })

  test('undetermined is a valid skip reason', () => {
    const reason: LanguageIdSkipReason = 'undetermined'
    expect(speechEventToControlMessage({ type: 'language_id_skipped', reason }).reason).toBe(
      'undetermined',
    )
  })
})
