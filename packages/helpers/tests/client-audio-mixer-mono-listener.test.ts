import { describe, expect, it, vi } from 'vitest'

import {
  ClientAudioMixer,
  type ClientMixGraph,
  type ClientMixerWarn,
} from '../src/client-audio-mixer.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'

function createGraph(): ClientMixGraph {
  const silence = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  const noop = () => undefined
  return {
    addInput: noop,
    removeInput: noop,
    pushFrame: noop,
    renderOutput: () => Buffer.from(silence),
    panTtsFrame: (pcm) => Buffer.from(pcm),
    setPose: noop,
    setPositionalEnabled: noop,
    setDefaultMixPlacement: noop,
    setTtsMixPlacement: noop,
    setTtsPose: noop,
    clearTtsPose: noop,
    setGroupMembers: noop,
    moveToGroup: noop,
    removeFromGroup: noop,
  }
}

function setup() {
  const warn = vi.fn<ClientMixerWarn>()
  const mixer = new ClientAudioMixer({ graph: createGraph(), warn, nativePump: false })
  return { warn, mixer }
}

describe('ClientAudioMixer mono-decoder listener warning', () => {
  it('mono listener in a positional mix warns once', () => {
    const { warn, mixer } = setup()
    mixer.registerPeer('client-a')
    mixer.setListenerReceivesStereo('client-a', false)
    expect(warn).not.toHaveBeenCalled()
    mixer.setPositionalEnabled(true)
    expect(warn).toHaveBeenCalledTimes(1)
    expect(warn.mock.calls[0][0]).toBe('mix_listener_mono_decoder peerId=client-a')
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(1)
    mixer.setPositionalEnabled(true)
    mixer.startMixPump('client-a', { writeSample: vi.fn(async () => undefined) })
    mixer.stopMixPump('client-a')
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(1)
    expect(warn).toHaveBeenCalledTimes(1)
  })

  it('stereo listener never warns', () => {
    const { mixer } = setup()
    mixer.registerPeer('client-a')
    mixer.setListenerReceivesStereo('client-a', true)
    mixer.setPositionalEnabled(true)
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(0)
  })

  it('unknown listener never warns', () => {
    const { mixer } = setup()
    mixer.registerPeer('client-a')
    mixer.setListenerReceivesStereo('client-a', null)
    mixer.setPositionalEnabled(true)
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(0)
  })

  it('mono listener with all-center placement does not warn until panning turns on', () => {
    const { mixer } = setup()
    mixer.registerPeer('client-a')
    mixer.setListenerReceivesStereo('client-a', false)
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(0)
    mixer.setDefaultMixPlacement('left')
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(1)
  })

  it('re-registered peer can warn again', () => {
    const { mixer } = setup()
    mixer.setPositionalEnabled(true)
    mixer.registerPeer('client-a')
    mixer.setListenerReceivesStereo('client-a', false)
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(1)
    mixer.unregisterPeer('client-a')
    mixer.registerPeer('client-a')
    mixer.setListenerReceivesStereo('client-a', false)
    expect(mixer.getMonoDecoderListenerWarnings()).toBe(2)
  })

  it('status snapshot carries listenerReceivesStereo', () => {
    const { mixer } = setup()
    mixer.registerPeer('client-a')
    expect(mixer.getMixSnapshot('client-a').listenerReceivesStereo).toBeNull()
    mixer.setListenerReceivesStereo('client-a', false)
    expect(mixer.getMixSnapshot('client-a').listenerReceivesStereo).toBe(false)
    mixer.setListenerReceivesStereo('client-a', true)
    expect(mixer.listMixSnapshots()[0].listenerReceivesStereo).toBe(true)
  })
})
