import { createServer, type Server, type Socket } from 'net'

import { afterEach, describe, expect, it } from 'vitest'

import { SignalingClient } from '../src'

/**
 * A `ws` socket that is closed while its handshake is still pending emits
 * `error` ("WebSocket was closed before the connection was established") on the next tick.
 * `disconnect()` and a replacing `connect()` drop the socket's listeners before closing it,
 * so that error used to surface as an uncaught exception (it failed the helpers test run when a
 * session pod tore down during a signaling reconnect).
 */

/** TCP server that accepts connections and never answers the WebSocket handshake. */
async function silentServer(): Promise<{
  server: Server
  url: string
  accepted: () => Promise<void>
  stop: () => Promise<void>
}> {
  const sockets: Socket[] = []
  let notify: (() => void) | null = null
  const server = createServer((socket) => {
    sockets.push(socket)
    notify?.()
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (address === null || typeof address === 'string') throw new Error('no port')
  const accepted = (): Promise<void> =>
    sockets.length > 0 ? Promise.resolve() : new Promise((resolve) => (notify = resolve))
  // Destroy the accepted sockets first: `server.close()` waits for open connections.
  const stop = (): Promise<void> => {
    sockets.forEach((socket) => socket.destroy())
    return new Promise((resolve) => server.close(() => resolve()))
  }
  return { server, url: `ws://127.0.0.1:${address.port}`, accepted, stop }
}

/** Collects uncaught exceptions for `ms` (the `ws` error is emitted on the next tick). */
async function uncaughtWithin(ms: number, action: () => void): Promise<Error[]> {
  const seen: Error[] = []
  const onError = (error: Error): void => {
    seen.push(error)
  }
  process.on('uncaughtException', onError)
  try {
    action()
    await new Promise((resolve) => setTimeout(resolve, ms))
  } finally {
    process.off('uncaughtException', onError)
  }
  return seen
}

describe('SignalingClient closing a socket whose handshake is pending', () => {
  let stop: (() => Promise<void>) | null = null

  afterEach(async () => {
    await stop?.()
    stop = null
  })

  it('disconnect() while connecting raises no uncaught error', async () => {
    const silent = await silentServer()
    stop = silent.stop
    const client = new SignalingClient({ url: silent.url, room: 'r', peerId: 'p' })
    // The promise never settles: the handshake is never answered and disconnect() drops it.
    client.connect().catch(() => {})
    await silent.accepted()

    const errors = await uncaughtWithin(100, () => client.disconnect())
    expect(errors.map((e) => e.message)).toEqual([])
  })

  it('a second connect() while the first is connecting raises no uncaught error', async () => {
    const silent = await silentServer()
    stop = silent.stop
    const client = new SignalingClient({ url: silent.url, room: 'r', peerId: 'p' })
    client.connect().catch(() => {})
    await silent.accepted()

    const errors = await uncaughtWithin(100, () => {
      client.connect().catch(() => {})
    })
    expect(errors.map((e) => e.message)).toEqual([])
    client.disconnect()
  })
})
