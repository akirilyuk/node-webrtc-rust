import { describe, expect, test } from 'vitest'

import { ttsEngineText } from '../src/voice'

describe('ttsEngineText', () => {
  test('capitalizes the sentence start and counts it', () => {
    expect(ttsEngineText('Okay. eight one six three ten')).toEqual({
      text: 'Okay. Eight one six three ten',
      adjusted: 1,
    })
  })

  test('leaves tiny fragments unchanged', () => {
    expect(ttsEngineText('I. am. happy.')).toEqual({ text: 'I. am. happy.', adjusted: 0 })
  })

  test("mode 'off' returns the text unchanged", () => {
    expect(ttsEngineText('Okay. eight one six three ten', 'off')).toEqual({
      text: 'Okay. eight one six three ten',
      adjusted: 0,
    })
  })

  test("mode 'auto' is the default", () => {
    expect(ttsEngineText('Okay. eight one', 'auto')).toEqual(ttsEngineText('Okay. eight one'))
  })
})
