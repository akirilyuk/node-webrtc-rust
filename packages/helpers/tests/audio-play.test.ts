import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mkdtempSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

import {
  UrlClipDiskCache,
  clipPlayInputId,
  scaleStereoPcmVolume,
  startClipPlayback,
} from '../src/clip-playback.js'
import { PCM_FULL_FRAME_BYTES } from '../src/pcm.js'
import type { ClientMixGraph } from '../src/client-audio-mixer.js'
import {
  AUDIO_PLAY_REQUIRES_VOICE,
  VoiceAgentSessionHost,
} from '../src/voice-agent-session-host.js'

const playerMocks = vi.hoisted(() => {
  const progressiveWriter = {
    append: vi.fn(),
    markEof: vi.fn(),
    isEof: vi.fn(() => false),
  }
  return {
    playClip: vi.fn(() => 'play-path'),
    playClipBytes: vi.fn(() => 'play-bytes'),
    playClipProgressive: vi.fn(() => ({ playId: 'play-prog', writer: progressiveWriter })),
    getClip: vi.fn(() => ({
      playId: 'play-prog',
      status: 'playing',
      positionMs: 0,
      bufferedMs: 100,
    })),
    stopClip: vi.fn(() => true),
    takeClipFrame: vi.fn(() => Buffer.alloc(PCM_FULL_FRAME_BYTES, 1)),
    progressiveWriter,
  }
})

vi.mock('@node-webrtc-rust/sdk/player', () => ({
  playClip: playerMocks.playClip,
  playClipBytes: playerMocks.playClipBytes,
  playClipProgressive: playerMocks.playClipProgressive,
  getClip: playerMocks.getClip,
  stopClip: playerMocks.stopClip,
  takeClipFrame: playerMocks.takeClipFrame,
}))

function createMockMixGraph(): ClientMixGraph & {
  listenerRoutes: Map<string, string[]>
} {
  const listenerRoutes = new Map<string, string[]>()
  const silence = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  return {
    listenerRoutes,
    addInput: vi.fn(),
    removeInput: vi.fn(),
    pushFrame: vi.fn(),
    renderOutput: vi.fn(() => Buffer.from(silence)),
    panTtsFrame: vi.fn((pcm: Buffer) => Buffer.from(pcm)),
    setPose: vi.fn(),
    setPositionalEnabled: vi.fn(),
    setDefaultMixPlacement: vi.fn(),
    setSourceMixPlacement: vi.fn(),
    clearSourceMixPlacement: vi.fn(),
    setTtsMixPlacement: vi.fn(),
    setTtsPose: vi.fn(),
    clearTtsPose: vi.fn(),
    setGroupMembers: vi.fn(),
    moveToGroup: vi.fn(),
    removeFromGroup: vi.fn(),
    listenerSources: (listener: string) => listenerRoutes.get(listener) ?? null,
    setListenerSources: vi.fn((listener: string, sources: string[]) => {
      listenerRoutes.set(listener, sources)
    }),
    clearListenerRoutes: vi.fn((listener: string) => {
      listenerRoutes.delete(listener)
    }),
  }
}

type HostTestAccess = VoiceAgentSessionHost & {
  sessions: Map<string, { agent?: { stop: () => Promise<void> }; agentStarted: boolean }>
}

function createHost(
  sessionMode: 'voice' | 'voice+data' | 'data-only',
  clientMixGraph?: ClientMixGraph,
): HostTestAccess {
  const host = new VoiceAgentSessionHost(createStubSignaling() as never, [], {
    voiceConfig: { stt: { provider: 'mock' }, tts: { provider: 'mock' } } as never,
    sessionMode,
    clientMixGraph,
    sessionBudget: {
      tryAcquire: () => 'lease-test',
      release: () => undefined,
      snapshot: () => ({ active: 0, max: 0, available: 0, rejectedTotal: 0 }),
    },
  })
  return host as unknown as HostTestAccess
}

function createStubSignaling() {
  return { room: 'test-room', on: vi.fn() }
}

describe('clip-playback', () => {
  let cacheDir: string

  beforeEach(() => {
    cacheDir = mkdtempSync(join(tmpdir(), 'clip-cache-'))
    vi.clearAllMocks()
  })

  afterEach(() => {
    rmSync(cacheDir, { recursive: true, force: true })
  })

  it('clipPlayInputId prefixes play id', () => {
    expect(clipPlayInputId('abc')).toBe('play:abc')
  })

  it('scaleStereoPcmVolume scales samples', () => {
    const frame = Buffer.alloc(4)
    frame.writeInt16LE(1000, 0)
    frame.writeInt16LE(-2000, 2)
    const scaled = scaleStereoPcmVolume(frame, 0.5)
    expect(scaled.readInt16LE(0)).toBe(500)
    expect(scaled.readInt16LE(2)).toBe(-1000)
  })

  it('startClipPlayback uses path and bytes bindings', async () => {
    await expect(startClipPlayback({ path: '/tmp/a.wav' })).resolves.toBe('play-path')
    await expect(startClipPlayback({ bytes: Buffer.from('abc') })).resolves.toBe('play-bytes')
    expect(playerMocks.playClip).toHaveBeenCalledWith('/tmp/a.wav')
    expect(playerMocks.playClipBytes).toHaveBeenCalled()
  })

  it('cache hit skips fetch on second URL load', async () => {
    const fetch = vi.fn(async () => new Uint8Array([1, 2, 3, 4]))
    const cache = new UrlClipDiskCache({ cacheDir, fetch })
    const first = await cache.loadUrlBytes('https://example.com/clip.wav')
    const second = await cache.loadUrlBytes('https://example.com/clip.wav')
    expect(first.fromCache).toBe(false)
    expect(second.fromCache).toBe(true)
    expect(fetch).toHaveBeenCalledTimes(1)
  })

  it('progressive URL streams chunks before fetch settles', async () => {
    const appendOrder: string[] = []
    const streamFetch = vi.fn(
      async (_url: string, onChunk: (chunk: Uint8Array) => void): Promise<Uint8Array> => {
        appendOrder.push('fetch-start')
        onChunk(new Uint8Array([1, 2, 3]))
        appendOrder.push('first-chunk')
        await new Promise((r) => setTimeout(r, 5))
        onChunk(new Uint8Array([4, 5]))
        appendOrder.push('second-chunk')
        return new Uint8Array([1, 2, 3, 4, 5])
      },
    )
    const cache = new UrlClipDiskCache({ cacheDir, streamFetch })
    const { playId, cacheDone } = await cache.startProgressiveUrl('https://example.com/stream.wav')
    expect(playId).toBe('play-prog')
    expect(playerMocks.progressiveWriter.append).toHaveBeenCalled()
    expect(appendOrder).toContain('first-chunk')
    await cacheDone
    expect(playerMocks.progressiveWriter.markEof).toHaveBeenCalled()
  })

  it('progressive URL uses cached path when present', async () => {
    const fetch = vi.fn(async () => new Uint8Array([9, 9, 9]))
    const cache = new UrlClipDiskCache({ cacheDir, fetch })
    await cache.loadUrlBytes('https://example.com/cached.wav')
    fetch.mockClear()
    const { playId } = await cache.startProgressiveUrl('https://example.com/cached.wav')
    expect(playId).toBe('play-path')
    expect(fetch).not.toHaveBeenCalled()
  })
})

describe('VoiceAgentSessionHost playAudio', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('rejects data-only sessions', async () => {
    const host = createHost('data-only')
    await expect(host.playAudio({ source: { bytes: Buffer.from('x') } })).rejects.toThrow(
      AUDIO_PLAY_REQUIRES_VOICE,
    )
  })

  it('throws for unknown peer id', async () => {
    const host = createHost('voice+data', createMockMixGraph())
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    await expect(
      host.playAudio({ source: { bytes: Buffer.from('x') }, peerIds: ['client-missing'] }),
    ).rejects.toThrow('No active voice session for client client-missing')
  })

  it('plays local bytes to one peer and wires mix input routes', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.sessions.set('client-b', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.getClientMixer()?.registerPeer('client-a')
    host.getClientMixer()?.registerPeer('client-b')

    const { playId } = await host.playAudio({
      source: { bytes: Buffer.from('wav') },
      peerIds: ['client-a'],
    })
    expect(playId).toBe('play-bytes')
    expect(graph.addInput).toHaveBeenCalledWith('play:play-bytes')
    expect(graph.setListenerSources).toHaveBeenCalledTimes(1)
    expect(graph.setListenerSources).toHaveBeenCalledWith(
      'client-a',
      expect.arrayContaining(['client-b', 'play:play-bytes']),
    )
    const routesA = graph.listenerRoutes.get('client-a') ?? []
    expect(routesA).toContain('play:play-bytes')
    expect(graph.listenerRoutes.has('client-b')).toBe(false)
  })

  it('does not call setListenerSources for non-target peers with implicit routes', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.sessions.set('client-b', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.getClientMixer()?.registerPeer('client-a')
    host.getClientMixer()?.registerPeer('client-b')

    await host.playAudio({
      source: { bytes: Buffer.from('wav') },
      peerIds: ['client-a'],
    })

    expect(graph.setListenerSources).toHaveBeenCalledTimes(1)
    expect(graph.setListenerSources).not.toHaveBeenCalledWith('client-b', expect.anything())
    expect(graph.listenerRoutes.get('client-a')).toContain('play:play-bytes')
    expect(graph.listenerRoutes.has('client-b')).toBe(false)
  })

  it('plays to all active voice clients when peerIds omitted', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice', graph)
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.getClientMixer()?.registerPeer('client-a')
    await host.playAudio({ source: { path: '/tmp/demo.wav' } })
    expect(playerMocks.playClip).toHaveBeenCalledWith('/tmp/demo.wav')
  })

  it('applies clip placement via setSourceMixPlacement', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.getClientMixer()?.registerPeer('client-a')

    await host.playAudio({
      source: { bytes: Buffer.from('wav') },
      peerIds: ['client-a'],
      position: { placement: 'left' },
    })
    expect(graph.setSourceMixPlacement).toHaveBeenCalledWith('play:play-bytes', 'left')
  })

  it('applies clip pose via setPose and enables positional mixing', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.getClientMixer()?.registerPeer('client-a')
    const pose = {
      position: { x: 2, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }

    await host.playAudio({
      source: { bytes: Buffer.from('wav') },
      peerIds: ['client-a'],
      position: { pose },
    })
    expect(graph.setPose).toHaveBeenCalledWith('play:play-bytes', pose)
    expect(graph.setPositionalEnabled).toHaveBeenCalledWith(true)
  })

  it('rejects playAudio with both placement and pose', async () => {
    const host = createHost('voice+data', createMockMixGraph())
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.getClientMixer()?.registerPeer('client-a')
    await expect(
      host.playAudio({
        source: { bytes: Buffer.from('wav') },
        peerIds: ['client-a'],
        position: {
          placement: 'left',
          pose: {
            position: { x: 0, y: 0, z: 0 },
            orientation: { x: 0, y: 0, z: 0, w: 1 },
          },
        },
      }),
    ).rejects.toThrow(/mutually exclusive/)
  })

  it('setTtsPosition routes to setTtsMixPlacement or setTtsPose', () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.setTtsPosition({ placement: 'right' })
    expect(graph.setTtsMixPlacement).toHaveBeenCalledWith('right')

    const pose = {
      position: { x: 1, y: 0, z: 0 },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
    }
    host.setTtsPosition({ pose }, { clientId: 'client-a' })
    expect(graph.setTtsPose).toHaveBeenCalledWith('client-a', pose)
  })

  it('setTtsPosition with pose requires clientId', () => {
    const host = createHost('voice+data', createMockMixGraph())
    expect(() =>
      host.setTtsPosition({
        pose: {
          position: { x: 0, y: 0, z: 0 },
          orientation: { x: 0, y: 0, z: 0, w: 1 },
        },
      }),
    ).toThrow(/clientId/)
  })

  it('stopAudioPlay stops native clip and removes mix input', async () => {
    const graph = createMockMixGraph()
    const host = createHost('voice+data', graph)
    host.sessions.set('client-a', {
      agent: { stop: vi.fn(async () => undefined) },
      agentStarted: true,
    })
    host.getClientMixer()?.registerPeer('client-a')
    const { playId } = await host.playAudio({
      source: { bytes: Buffer.from('wav') },
      peerIds: ['client-a'],
    })
    expect(host.stopAudioPlay(playId)).toBe(true)
    expect(playerMocks.stopClip).toHaveBeenCalledWith(playId)
    expect(graph.removeInput).toHaveBeenCalledWith(`play:${playId}`)
  })
})
