import { createServer, request, Agent } from 'http'
import net from 'net'

import { describe, expect, it } from 'vitest'
import { WebSocket } from 'ws'

import { SignalingServer } from '../src/server'

type Mode = 'own' | 'attached'

async function start(mode: Mode, closeTimeoutMs: number): Promise<SignalingServer> {
  const server =
    mode === 'attached'
      ? new SignalingServer({ server: createServer(), closeTimeoutMs, pingIntervalMs: 0 })
      : new SignalingServer({ closeTimeoutMs, pingIntervalMs: 0 })
  await server.listen(0, '127.0.0.1')
  return server
}

function opened(url: string): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url)
    ws.once('open', () => resolve(ws))
    ws.once('error', reject)
  })
}

for (const mode of ['own', 'attached'] as Mode[]) {
  describe(`SignalingServer.close bounded (${mode} http server)`, () => {
    it('close resolves when a client connected but never joined', async () => {
      const server = await start(mode, 300)
      const client = await opened(`ws://127.0.0.1:${server.port}`)
      const closed = new Promise<number>((resolve) => client.once('close', (code) => resolve(code)))
      const t0 = Date.now()
      await server.close()
      expect(Date.now() - t0).toBeLessThan(300 + 500)
      expect(await closed).toBe(1001)
    })

    it('close resolves within closeTimeoutMs when a client never answers the close frame', async () => {
      const server = await start(mode, 300)
      const client = await opened(`ws://127.0.0.1:${server.port}`)
      // Stop reading so the close handshake never completes from the client side.
      ;(client as unknown as { _socket: net.Socket })._socket.pause()
      const t0 = Date.now()
      await server.close()
      expect(Date.now() - t0).toBeLessThan(1500)
      client.terminate()
    })

    it('close still closes room peers normally', async () => {
      const server = await start(mode, 2000)
      const client = await opened(`ws://127.0.0.1:${server.port}`)
      const closed = new Promise<number>((resolve) => client.once('close', (code) => resolve(code)))
      client.send(JSON.stringify({ type: 'join', room: 'r1', peerId: 'p1' }))
      await new Promise((r) => setTimeout(r, 100))
      const t0 = Date.now()
      await server.close()
      expect(Date.now() - t0).toBeLessThan(500)
      expect(await closed).toBe(1001)
    })
  })
}

describe('SignalingServer.close with idle keep-alive HTTP connection', () => {
  it('resolves within the bound (attached server)', async () => {
    const http = createServer((_req, res) => {
      res.end('ok')
    })
    const server = new SignalingServer({ server: http, closeTimeoutMs: 300, pingIntervalMs: 0 })
    await server.listen(0, '127.0.0.1')
    const agent = new Agent({ keepAlive: true })
    await new Promise<void>((resolve, reject) => {
      const req = request({ host: '127.0.0.1', port: server.port, agent, path: '/' }, (res) => {
        res.resume()
        res.on('end', () => resolve())
      })
      req.on('error', reject)
      req.end()
    })
    const t0 = Date.now()
    await server.close()
    expect(Date.now() - t0).toBeLessThan(1500)
    agent.destroy()
  })
})
