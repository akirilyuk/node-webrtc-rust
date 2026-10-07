import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'

import { afterEach, describe, expect, test } from 'vitest'
import WebSocket from 'ws'

import { SignalingServer } from '../src'

/**
 * A wildcard listener (`::` / `0.0.0.0`) can be shadowed on some operating systems: a second
 * server may bind the same port on `127.0.0.1` specifically, and loopback connections then go to
 * the more specific socket. Test suites run files in parallel and start many loopback servers on
 * ephemeral ports, so a signaling test that connects to `127.0.0.1:<port>` could reach an
 * unrelated HTTP server (404 / 401 instead of a WebSocket upgrade).
 */

const servers: Server[] = []
const signaling: SignalingServer[] = []

afterEach(async () => {
  for (const server of servers.splice(0)) {
    await new Promise<void>((resolve) => server.close(() => resolve()))
  }
  for (const server of signaling.splice(0)) {
    await server.close()
  }
})

/** Tries to bind a plain HTTP server answering 401 on `127.0.0.1:<port>`. Resolves `null` on EADDRINUSE. */
async function tryBindLoopbackShadow(port: number): Promise<Server | null> {
  const shadow = createServer((_req, res) => {
    res.statusCode = 401
    res.end()
  })
  servers.push(shadow)
  return new Promise<Server | null>((resolve, reject) => {
    shadow.once('error', (error: NodeJS.ErrnoException) => {
      if (error.code === 'EADDRINUSE') {
        resolve(null)
        return
      }
      reject(error)
    })
    shadow.listen(port, '127.0.0.1', () => resolve(shadow))
  })
}

function connectOutcome(port: number): Promise<string> {
  return new Promise((resolve) => {
    const socket = new WebSocket(`ws://127.0.0.1:${port}/ws`)
    socket.once('open', () => {
      socket.close()
      resolve('open')
    })
    socket.once('error', (error) => resolve(error.message))
  })
}

describe('SignalingServer loopback binding', () => {
  test('wildcard listen(0) can be shadowed by a 127.0.0.1 bind on the same port (OS-dependent)', async (ctx) => {
    const server = new SignalingServer({ pingIntervalMs: 0 })
    signaling.push(server)
    await server.listen(0)

    const shadow = await tryBindLoopbackShadow(server.port)
    if (!shadow) {
      ctx.skip(
        'this OS rejects a 127.0.0.1 bind on a port already held by a wildcard listener (EADDRINUSE); shadowing hazard not reproducible here',
      )
      return
    }

    const outcome = await connectOutcome(server.port)
    expect(outcome).toBe('Unexpected server response: 401')
  })

  test('listen(0, "127.0.0.1") cannot be shadowed and accepts the WebSocket upgrade', async () => {
    const server = new SignalingServer({ pingIntervalMs: 0 })
    signaling.push(server)
    await server.listen(0, '127.0.0.1')

    const address = (
      server as unknown as { httpServer?: Server }
    ).httpServer?.address() as AddressInfo
    expect(address.address).toBe('127.0.0.1')

    const shadow = await tryBindLoopbackShadow(server.port)
    expect(shadow).toBeNull()

    const outcome = await connectOutcome(server.port)
    expect(outcome).toBe('open')
  })
})
