/**
 * Browser + Node voice agent demo.
 *
 * Serves a static page and WebSocket signaling. Each browser tab connects as a
 * WebRTC client; the Node server runs VoiceAgent (mock vendors by default).
 *
 * Mock (default):
 *   npm run start --workspace=@node-webrtc-rust/example-voice-agent-browser
 *
 * Live cloud vendor (set API keys — see README):
 *   OPENAI_API_KEY=sk-... npm run start:live:openai --workspace=@node-webrtc-rust/example-voice-agent-browser
 *
 * Open http://localhost:3001 — allow microphone, connect, speak, and use the
 * form to request TTS. Try "Speak long reply" then talk over it to see barge_in.
 */

import { createServer, type IncomingMessage, type ServerResponse } from 'http'
import { readFile } from 'fs/promises'
import { dirname, extname, join } from 'path'
import { fileURLToPath } from 'url'

import { SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import { resolveVoiceConfig } from './resolve-voice-config.js'
import { SERVER_PEER_ID, VoiceAgentSessionHost } from './session-host.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const SHARED_DIR = join(__dirname, '../../shared')
const PORT = Number(process.env.PORT ?? 3001)
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
}

/** Resolved once at startup — mock unless VOICE_VENDOR is set (see README). */
const { config: voiceConfig, label: voiceLabel, mode: voiceMode } = resolveVoiceConfig()

interface ActiveRoom {
  signaling: SignalingClient
  host: VoiceAgentSessionHost
}

const rooms = new Map<string, ActiveRoom>()

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

async function ensureRoom(room: string): Promise<void> {
  if (rooms.has(room)) return

  const signaling = new SignalingClient({
    url: `ws://127.0.0.1:${PORT}/ws`,
    room,
    peerId: SERVER_PEER_ID,
  })
  await signaling.connect()

  const host = new VoiceAgentSessionHost(signaling, ICE_SERVERS, { voiceConfig })
  rooms.set(room, { signaling, host })
  console.log(`Room ready: ${room}`)
}

async function main(): Promise<void> {
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
        await ensureRoom(room)
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

  const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
  await signaling.listen(PORT)

  console.log(`Voice agent browser demo at http://localhost:${PORT}`)
  console.log(`Signaling: ws://localhost:${PORT}/ws`)
  console.log(
    `Voice pipeline: ${voiceLabel} — STT=${voiceConfig.stt?.provider}, TTS=${voiceConfig.tts?.provider}`,
  )
  if (voiceMode === 'mock') {
    console.log(
      'Using mock STT/TTS. Set VOICE_VENDOR and API keys for live vendors — see README.',
    )
  }
  console.log('Allow microphone access, connect, speak, and watch STT events in the log.')
  console.log('Use the TTS form or "Speak long reply" to test barge-in while talking.')

  const shutdown = async () => {
    for (const active of rooms.values()) {
      await active.host.close()
      active.signaling.disconnect()
    }
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
