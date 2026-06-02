import { createServer, type IncomingMessage, type ServerResponse } from 'http'
import { readFile } from 'fs/promises'
import { dirname, extname, join } from 'path'
import { fileURLToPath } from 'url'

import { SignalingServer } from '@node-webrtc-rust/signaling'

import { freePort } from '../../shared/free-port.js'
import { RoomManager } from './room-manager'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const SHARED_DIR = join(__dirname, '../../shared')
const PORT = Number(process.env.PORT ?? 3000)
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]
const TONE_HZ = 440

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
}

async function serveStatic(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const pathname = req.url?.split('?')[0] ?? '/'

  if (pathname.startsWith('/shared/')) {
    const sharedPath = join(SHARED_DIR, pathname.slice('/shared/'.length))
    const ext = extname(sharedPath)
    try {
      const body = await readFile(sharedPath)
      res.writeHead(200, { 'Content-Type': MIME_TYPES[ext] ?? 'application/octet-stream' })
      res.end(body)
      return
    } catch {
      res.writeHead(404)
      res.end('Not found')
      return
    }
  }

  const filePath = join(PUBLIC_DIR, pathname === '/' ? 'index.html' : pathname)
  const ext = extname(filePath)

  try {
    const body = await readFile(filePath)
    res.writeHead(200, { 'Content-Type': MIME_TYPES[ext] ?? 'application/octet-stream' })
    res.end(body)
  } catch {
    res.writeHead(404)
    res.end('Not found')
  }
}

async function main(): Promise<void> {
  const roomManager = new RoomManager({
    signalingUrl: `ws://127.0.0.1:${PORT}/ws`,
    iceServers: ICE_SERVERS,
    frequencyHz: TONE_HZ,
  })

  const httpServer = createServer(async (req, res) => {
    if (req.method === 'POST' && req.url === '/api/rooms') {
      let body = ''
      for await (const chunk of req) {
        body += chunk
      }
      try {
        const { room } = JSON.parse(body) as { room?: string }
        if (!room || typeof room !== 'string') {
          res.writeHead(400, { 'Content-Type': 'application/json' })
          res.end(JSON.stringify({ error: 'room is required' }))
          return
        }
        await roomManager.ensureRoom(room)
        res.writeHead(200, { 'Content-Type': 'application/json' })
        res.end(JSON.stringify({ room }))
      } catch (error) {
        res.writeHead(500, { 'Content-Type': 'application/json' })
        res.end(JSON.stringify({ error: String(error) }))
      }
      return
    }

    await serveStatic(req, res)
  })

  freePort(PORT, 'browser-cosine-chat')

  const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
  await signaling.listen(PORT)

  console.log(`Browser cosine + chat example running at http://localhost:${PORT}`)
  console.log(`WebSocket signaling: ws://localhost:${PORT}/ws`)
  console.log(`Open multiple browser tabs, pick a room, and connect.`)
  if (process.env.TONE_STREAM_DEBUG === '1') {
    console.log(
      'TONE_STREAM_DEBUG=1 — PCM tick logs also written to examples/browser-cosine-chat/tone-debug.log',
    )
    console.log('  tail -f examples/browser-cosine-chat/tone-debug.log')
  } else {
    console.log(
      'Run npm run start:debug (or TONE_STREAM_DEBUG=1) to log PCM ticks to tone-debug.log',
    )
  }

  const shutdown = async () => {
    await roomManager.close()
    await signaling.close()
    process.exit(0)
  }

  process.on('SIGINT', () => void shutdown())
  process.on('SIGTERM', () => void shutdown())
}

void main().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
