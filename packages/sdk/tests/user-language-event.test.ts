import { describe, expect, test } from 'vitest'

import { SPEECH_EVENT_TYPE, type LanguageIdSkipReason } from '../src/voice/types'
import { JsSpeechEventType } from '@node-webrtc-rust/bindings'

import { speechEventToControlMessage } from '../src/voice/speech-event-bridge'
import { fromJsSpeechEvent } from '../src/voice/VoiceAgent'

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
      utteranceId: 'utt-1',
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

describe('stt hold fields on the wire', () => {
  test('stt_hold_started carries holdMode and bufferedMs', () => {
    const msg = speechEventToControlMessage({
      type: 'stt_hold_started',
      holdMode: 'buffer_replay',
      bufferedMs: 320,
    })
    expect(msg.holdMode).toBe('buffer_replay')
    expect(msg.bufferedMs).toBe(320)
  })

  test('stt_hold_ended carries holdOutcome, bufferedMs and droppedMs', () => {
    const msg = speechEventToControlMessage({
      type: 'stt_hold_ended',
      holdOutcome: 'released_replay',
      bufferedMs: 900,
      droppedMs: 40,
    })
    expect(msg.holdOutcome).toBe('released_replay')
    expect(msg.bufferedMs).toBe(900)
    expect(msg.droppedMs).toBe(40)
  })

  test('utterance and replay fields are forwarded', () => {
    const msg = speechEventToControlMessage({
      type: 'user_speech_final',
      text: 'hi',
      utteranceId: 'u2',
      replay: true,
      replacesUtteranceId: 'u1',
      languageMismatch: true,
    })
    expect(msg).toMatchObject({
      utteranceId: 'u2',
      replay: true,
      replacesUtteranceId: 'u1',
      languageMismatch: true,
    })
  })
})

describe('agent_speaking_start timing on the wire', () => {
  test('forwards firstChunkMs and firstAudioMs when set', () => {
    const msg = speechEventToControlMessage({
      type: 'agent_speaking_start',
      firstChunkMs: 180,
      firstAudioMs: 240,
    })
    expect(msg.firstChunkMs).toBe(180)
    expect(msg.firstAudioMs).toBe(240)
  })

  test('leaves them unset when the event carries no timing', () => {
    const msg = speechEventToControlMessage({ type: 'agent_speaking_start' })
    expect(msg.firstChunkMs).toBeUndefined()
    expect(msg.firstAudioMs).toBeUndefined()
    expect(JSON.parse(JSON.stringify(msg))).not.toHaveProperty('firstAudioMs')
  })
})

describe('tts_phrase_cache_hit speech events', () => {
  test('SPEECH_EVENT_TYPE includes tts_phrase_cache_hit', () => {
    expect(SPEECH_EVENT_TYPE.ttsPhraseCacheHit).toBe('tts_phrase_cache_hit')
  })

  test('native event maps to tts_phrase_cache_hit and passes chars through', () => {
    const event = fromJsSpeechEvent({
      eventType: JsSpeechEventType.TtsPhraseCacheHit,
      chars: 12,
    })
    expect(event.type).toBe('tts_phrase_cache_hit')
    expect(event.chars).toBe(12)
    expect(event.text).toBeUndefined()
  })

  test('speechEventToControlMessage forwards chars', () => {
    expect(speechEventToControlMessage({ type: 'tts_phrase_cache_hit', chars: 12 })).toMatchObject({
      type: 'speech_event',
      event: 'tts_phrase_cache_hit',
      chars: 12,
    })
  })
})
