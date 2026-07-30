import { describe, expect, it } from 'vitest'

import {
  evaluateRecognizedContainsWords,
  evaluateStreamingFasterOrEqual,
} from './roundtrip-tts-stream-helpers.js'

describe('evaluateStreamingFasterOrEqual', () => {
  it('passes when streaming beats buffered', () => {
    const result = evaluateStreamingFasterOrEqual({
      streamingMs: 200,
      bufferedMs: 800,
      minImprovementMs: 40,
      maxRegressionMs: 0,
    })
    expect(result.ok).toBe(true)
    expect(result.failures).toEqual([])
  })

  it('allows small regression within maxRegressionMs (Piper-low jitter)', () => {
    const result = evaluateStreamingFasterOrEqual({
      streamingMs: 172,
      bufferedMs: 166,
      minImprovementMs: 0,
      maxRegressionMs: 80,
    })
    expect(result.ok).toBe(true)
  })

  it('fails when streaming regresses beyond the jitter budget', () => {
    const result = evaluateStreamingFasterOrEqual({
      streamingMs: 300,
      bufferedMs: 166,
      minImprovementMs: 0,
      maxRegressionMs: 80,
    })
    expect(result.ok).toBe(false)
    expect(result.failures[0]).toMatch(/exceeds limit/)
  })

  it('fails when a required improvement margin is not met', () => {
    const result = evaluateStreamingFasterOrEqual({
      streamingMs: 780,
      bufferedMs: 800,
      minImprovementMs: 40,
      maxRegressionMs: 0,
    })
    expect(result.ok).toBe(false)
  })
})

describe('evaluateRecognizedContainsWords', () => {
  it('passes when required words appear', () => {
    const result = evaluateRecognizedContainsWords({
      recognized: 'Why do you need so much time until you start speaking',
      requiredWords: ['why', 'need', 'time', 'speaking'],
      label: 'streaming',
    })
    expect(result.ok).toBe(true)
  })

  it('fails when a required word is missing', () => {
    const result = evaluateRecognizedContainsWords({
      recognized: 'hello world',
      requiredWords: ['why', 'need'],
      label: 'buffered',
    })
    expect(result.ok).toBe(false)
    expect(result.failures.length).toBe(2)
  })
})
