/**
 * Browser + Node local Sherpa STT demo.
 *
 * Prerequisites:
 *   npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   export SHERPA_STT_MODEL_PATH=.../examples/voice-agent-local-sherpa/.models/<stt-bundle>
 *   export SHERPA_TTS_MODEL_PATH=.../examples/voice-agent-local-sherpa/.models/<tts-bundle>
 *
 * Run:
 *   npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *
 * Open http://localhost:3002 — allow microphone, connect, speak.
 * Partial/final transcripts arrive on the voice-control DataChannel.
 * Local Sherpa TTS (Piper/VITS) plays agent replies — no cloud keys.
 */

import { createServer, type IncomingMessage, type ServerResponse } from 'http'
import { readFile } from 'fs/promises'
import { dirname, extname, join } from 'path'
import { fileURLToPath } from 'url'

import { SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'
import { SERVER_PEER_ID, VoiceAgentSessionHost } from '@node-webrtc-rust/helpers'

import { resolveVoiceConfig } from './resolve-voice-config.js'
import { isVoiceDebugEnabled } from '@node-webrtc-rust/sdk/voice'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const SHARED_DIR = join(__dirname, '../../shared')
const PORT = Number(process.env.PORT ?? 3002)
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
}

const {
  config: voiceConfig,
  label: voiceLabel,
  sttModelPath,
  ttsModelPath,
  language,
} = resolveVoiceConfig()

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

  console.log(`Local Sherpa voice demo at http://localhost:${PORT}`)
  console.log(`Signaling: ws://localhost:${PORT}/ws`)
  console.log(`Voice pipeline: ${voiceLabel}`)
  console.log(`STT=local-sherpa (${language})  TTS=local-sherpa`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log('Allow microphone, connect, speak — watch partial/final STT in the event log.')
  console.log('Use the Speak form to hear on-device Piper TTS and test barge-in.')
  if (isVoiceDebugEnabled()) {
    console.error('[voice-debug] VOICE_DEBUG=1 — verbose pipeline logs on stderr')
    console.error('[voice-debug] WEBRTC_DEBUG=' + (process.env.WEBRTC_DEBUG ?? '0'))
    console.error('[voice-debug] Optional: VOICE_VAD_THRESHOLD=0.01  VOICE_VAD_DISABLED=1')
    console.error(
      '[voice-debug] Start with: npm run start:debug --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa',
    )
  }

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
