import { describe, expect, test } from 'vitest'

import { SPEECH_EVENT_TYPE, type SpeechEvent, type SpeechEventType } from '../src/voice/types'

describe('voice language switch SDK types', () => {
  test('SPEECH_EVENT_TYPE includes runner-emitted voice_language_* kinds', () => {
    const types: SpeechEventType[] = [
      SPEECH_EVENT_TYPE.voiceLanguageSwitching,
      SPEECH_EVENT_TYPE.voiceLanguageChanged,
      SPEECH_EVENT_TYPE.voiceLanguageSwitchFailed,
    ]
    expect(types).toEqual([
      'voice_language_switching',
      'voice_language_changed',
      'voice_language_switch_failed',
    ])
  })

  test('SpeechEvent optional replay and utterance fields type-check', () => {
    const event: SpeechEvent = {
      type: 'user_speech_final',
      text: 'hallo',
      utteranceId: 'utt-9',
      replay: true,
      replacesUtteranceId: 'utt-8',
    }
    expect(event.replay).toBe(true)
  })
})
