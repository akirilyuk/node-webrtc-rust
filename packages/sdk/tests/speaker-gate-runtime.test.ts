import { describe, expect, test, vi } from 'vitest'

import { VoiceAgent } from '../src/voice/VoiceAgent'

/** A VoiceAgent whose native handle is a stub (no native construction). */
function agentWithNative(native: { setSpeakerGate: (u: unknown) => void }): VoiceAgent {
  const agent = Object.create(VoiceAgent.prototype) as VoiceAgent
  ;(agent as unknown as { native: unknown }).native = native
  return agent
}

describe('VoiceAgent.setSpeakerGate', () => {
  test('forwards enabled, onMismatch and bargeIn to native', () => {
    const setSpeakerGate = vi.fn()
    const agent = agentWithNative({ setSpeakerGate })
    agent.setSpeakerGate({ enabled: true, onMismatch: 'drop', bargeIn: 'anyone' })
    expect(setSpeakerGate).toHaveBeenCalledTimes(1)
    expect(setSpeakerGate).toHaveBeenCalledWith({
      enabled: true,
      onMismatch: 'drop',
      bargeIn: 'anyone',
    })
  })

  test('omitted fields stay undefined', () => {
    const setSpeakerGate = vi.fn()
    agentWithNative({ setSpeakerGate }).setSpeakerGate({ enabled: false })
    expect(setSpeakerGate).toHaveBeenCalledWith({
      enabled: false,
      onMismatch: undefined,
      bargeIn: undefined,
    })
  })

  test('throws when native throws', () => {
    const agent = agentWithNative({
      setSpeakerGate: () => {
        throw new Error('config error: speakerGate.modelPath not set')
      },
    })
    expect(() => agent.setSpeakerGate({ enabled: true })).toThrow('speakerGate.modelPath not set')
  })
})
