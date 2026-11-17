import { beforeEach, describe, expect, it, vi } from 'vitest'

const hostCalls: Array<{ sessionId: string; iceServers: unknown }> = []
const connectGates = new Map<string, Promise<void>>()

vi.mock('@node-webrtc-rust/signaling', () => ({
  SignalingClient: class {
    readonly room: string
    constructor(opts: { room: string }) {
      this.room = opts.room
    }
    async connect(): Promise<void> {
      const gate = connectGates.get(this.room)
      if (gate) await gate
    }
    on(): this {
      return this
    }
  },
}))

vi.mock('../src/voice-agent-session-host.js', () => ({
  VOICE_AGENT_SERVER_PEER_ID: 'voice-agent-server',
  VoiceAgentSessionHost: class {
    constructor(signaling: { room: string }, iceServers: unknown) {
      hostCalls.push({ sessionId: signaling.room, iceServers })
    }
  },
}))

import { SessionPod } from '../src/session-pod.js'
import { SessionPodIceServersResolverError } from '../src/session-pod-errors.js'

function makePod(iceServers: ConstructorParameters<typeof SessionPod>[1]['iceServers']) {
  return new SessionPod({} as never, {
    signalingUrl: 'ws://127.0.0.1:1/ws',
    iceServers,
    voiceConfig: {} as never,
    sessionMode: 'data-only',
    teardownIdleSessions: false,
    log: () => undefined,
  })
}

function deferred<T>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe('SessionPod iceServers', () => {
  beforeEach(() => {
    hostCalls.length = 0
    connectGates.clear()
  })

  it('array form is passed to every session unchanged', async () => {
    const servers = [{ urls: 'ice-test-shared' }]
    const pod = makePod(servers)
    await pod.ensureSession('a')
    await pod.ensureSession('b')
    expect(hostCalls).toEqual([
      { sessionId: 'a', iceServers: servers },
      { sessionId: 'b', iceServers: servers },
    ])
    expect(hostCalls[0]!.iceServers).toBe(servers)
  })

  it('resolver is called once per session with that session id', async () => {
    const resolver = vi.fn(async (id: string) => [
      { urls: 'ice-test-per-session', username: `u:${id}`, credential: `c:${id}` },
    ])
    const pod = makePod(resolver)
    await pod.ensureSession('s1')
    await pod.ensureSession('s2')
    expect(resolver.mock.calls).toEqual([['s1'], ['s2']])
    expect(hostCalls.map((c) => c.sessionId)).toEqual(['s1', 's2'])
    expect((hostCalls[1]!.iceServers as Array<{ username: string }>)[0]!.username).toBe('u:s2')
  })

  it('supports a synchronous resolver', async () => {
    const pod = makePod((id) => [{ urls: `ice-test-${id}` }])
    await pod.ensureSession('sync-1')
    expect(hostCalls[0]!.iceServers).toEqual([{ urls: 'ice-test-sync-1' }])
  })

  it('concurrent sessions each get their own servers (interleave enforced)', async () => {
    const firstResolverGate = deferred<void>()
    const firstConnectGate = deferred<void>()
    connectGates.set('first', firstConnectGate.promise)
    const entered: string[] = []
    const pod = makePod(async (id) => {
      entered.push(id)
      // Park the first session inside its resolver until the second has fully prepared.
      if (id === 'first') await firstResolverGate.promise
      return [{ urls: 'ice-test-per-session', username: `u:${id}`, credential: `c:${id}` }]
    })

    const first = pod.ensureSession('first')
    await Promise.resolve()
    expect(entered).toEqual(['first'])

    // Second session starts and finishes while the first is parked.
    await pod.ensureSession('second')
    expect(hostCalls.map((c) => c.sessionId)).toEqual(['second'])

    firstResolverGate.resolve()
    // The first session is now parked at its signaling connect, a second await point.
    await new Promise((r) => setImmediate(r))
    expect(hostCalls.map((c) => c.sessionId)).toEqual(['second'])
    firstConnectGate.resolve()
    await first

    const byId = Object.fromEntries(
      hostCalls.map((c) => [
        c.sessionId,
        (c.iceServers as Array<{ username: string }>)[0]!.username,
      ]),
    )
    expect(byId).toEqual({ first: 'u:first', second: 'u:second' })
  })

  it('session parked at signaling connect keeps its own servers while another session prepares', async () => {
    const firstConnectGate = deferred<void>()
    connectGates.set('first', firstConnectGate.promise)
    const pod = makePod(async (id) => [
      { urls: 'ice-test-per-session', username: `u:${id}`, credential: `c:${id}` },
    ])

    const first = pod.ensureSession('first')
    await new Promise((r) => setImmediate(r))
    // First has resolved its servers and is parked at signaling.connect().
    expect(hostCalls).toEqual([])

    await pod.ensureSession('second')
    expect(hostCalls.map((c) => c.sessionId)).toEqual(['second'])

    firstConnectGate.resolve()
    await first
    const byId = Object.fromEntries(
      hostCalls.map((c) => [
        c.sessionId,
        (c.iceServers as Array<{ username: string }>)[0]!.username,
      ]),
    )
    expect(byId).toEqual({ first: 'u:first', second: 'u:second' })
  })

  it('resolver rejection fails only that session with a named error', async () => {
    const pod = makePod(async (id) => {
      if (id === 'bad') throw new Error('mint failed')
      return [{ urls: 'ice-test-shared' }]
    })
    const bad = pod.ensureSession('bad')
    const good = pod.ensureSession('good')
    await expect(bad).rejects.toBeInstanceOf(SessionPodIceServersResolverError)
    await expect(bad).rejects.toMatchObject({
      code: 'ice_servers_resolver_failed',
      sessionId: 'bad',
    })
    await expect(bad).rejects.toThrow(/ice_servers_resolver_failed: mint failed/)
    await good
    expect(hostCalls.map((c) => c.sessionId)).toEqual(['good'])
    expect(pod.activeSessionCount).toBe(1)
    // The pod keeps serving new sessions after the failure.
    await pod.ensureSession('later')
    expect(pod.activeSessionCount).toBe(2)
  })

  it('synchronous resolver throw fails only that session', async () => {
    const pod = makePod((id) => {
      if (id === 'bad') throw new Error('boom')
      return []
    })
    await expect(pod.ensureSession('bad')).rejects.toMatchObject({
      code: 'ice_servers_resolver_failed',
    })
    await pod.ensureSession('ok')
    expect(pod.activeSessionCount).toBe(1)
  })
})
