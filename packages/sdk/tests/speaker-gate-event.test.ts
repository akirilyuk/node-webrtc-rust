import { describe, expect, test } from 'vitest'

import { JsSpeechEventType } from '@node-webrtc-rust/bindings'

import { fromJsSpeechEvent } from '../src/voice/VoiceAgent'
import {
  SPEAKER_GATE_RECHECK_MAX_WINDOWS,
  SPEAKER_GATE_RECHECK_WINDOWS,
  speakerRecheckBargeEarliestMs,
} from '../src/voice/index'

describe('speaker gate speech events', () => {
  test('user_speech_ignored carries reason and utterance id, no text', () => {
    const ev = fromJsSpeechEvent({
      eventType: JsSpeechEventType.UserSpeechIgnored,
      reason: 'speaker_mismatch',
      utteranceId: 'u1',
    })
    expect(ev.type).toBe('user_speech_ignored')
    expect(ev.reason).toBe('speaker_mismatch')
    expect(ev.utteranceId).toBe('u1')
    expect(ev.text).toBeUndefined()
  })

  test('barge_in passes the utterance id through', () => {
    const ev = fromJsSpeechEvent({
      eventType: JsSpeechEventType.BargeIn,
      utteranceId: 'utt-7',
    })
    expect(ev.type).toBe('barge_in')
    expect(ev.utteranceId).toBe('utt-7')
  })

  test('user_speech_final passes the speaker verdict through', () => {
    const ev = fromJsSpeechEvent({
      eventType: JsSpeechEventType.UserSpeechFinal,
      text: 'hello',
      speaker: 'other',
    })
    expect(ev.type).toBe('user_speech_final')
    expect(ev.speaker).toBe('other')
    expect(ev.speakerUnknownReason).toBeUndefined()
  })

  test('user_speech_final passes speakerUnknownReason through', () => {
    const ev = fromJsSpeechEvent({
      eventType: JsSpeechEventType.UserSpeechFinal,
      text: 'stop',
      speaker: 'unknown',
      speakerUnknownReason: 'short',
    })
    expect(ev.speaker).toBe('unknown')
    expect(ev.speakerUnknownReason).toBe('short')
  })

  test('user_speech_final passes speakerRechecked through, absent when unset', () => {
    const ev = fromJsSpeechEvent({
      eventType: JsSpeechEventType.UserSpeechFinal,
      text: 'hi',
      speaker: 'caller',
      speakerRechecked: true,
    })
    expect(ev.speakerRechecked).toBe(true)
    const plain = fromJsSpeechEvent({
      eventType: JsSpeechEventType.UserSpeechFinal,
      text: 'hi',
      speaker: 'other',
    })
    expect(plain.speakerRechecked).toBeUndefined()
  })

  test('re-check bound mirrors the speech crate (RECHECK_WINDOWS = 2)', () => {
    expect(SPEAKER_GATE_RECHECK_WINDOWS).toBe(2)
    expect(speakerRecheckBargeEarliestMs(1500, 2)).toBe(3000)
  })

  test('barge-in bound is checkSpeechMs x window', () => {
    expect(speakerRecheckBargeEarliestMs(1500, 2)).toBe(3000)
    expect(speakerRecheckBargeEarliestMs(1500, 8)).toBe(12000)
  })

  test('default window cap mirrors DEFAULT_RECHECK_MAX_WINDOWS in speaker_gate.rs', () => {
    expect(SPEAKER_GATE_RECHECK_MAX_WINDOWS).toBe(8)
  })

  test('speakerCheckWindows maps from the native event', () => {
    const ev = fromJsSpeechEvent({
      eventType: JsSpeechEventType.UserSpeechFinal,
      text: 'hi',
      speaker: 'caller',
      speakerCheckWindows: 3,
    })
    expect(ev.speakerCheckWindows).toBe(3)
    const plain = fromJsSpeechEvent({
      eventType: JsSpeechEventType.UserSpeechFinal,
      text: 'hi',
    })
    expect(plain.speakerCheckWindows).toBeUndefined()
  })
})
