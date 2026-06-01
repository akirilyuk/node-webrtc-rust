/**
 * Local Sherpa — **three clients, one room**, shared native models.
 *
 * **Your app logic:** edit `src/voice-handler.ts` (`onSpeechEvent`, `onSpeakRequest`).
 *
 * Demonstrates {@link startMultiClientVoiceServer}: one process, one room, one
 * `VoiceAgent` per browser tab; Sherpa weights pooled in Rust.
 *
 * Prerequisites (same as voice-agent-local-sherpa):
 *   npm run download-stt --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   npm run download-tts --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa
 *   export SHERPA_STT_MODEL_PATH=...
 *   export SHERPA_TTS_MODEL_PATH=...
 *
 * Run:
 *   npm run start --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
 *
 * Open **three browser tabs** to http://localhost:3004 — same room `sherpa-multi`.
 * Optional: `VOICE_MAX_CONCURRENT_SESSIONS=2 npm run start:cap-2 --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client`
 * then the third tab should fail to negotiate (check server log + GET /api/capacity).
 *
 * Tests (no models required):
 *   npm run test --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
 */

import { readFile } from 'fs/promises'
import { dirname, extname, join } from 'path'
import { fileURLToPath } from 'url'
import type { IncomingMessage, ServerResponse } from 'http'

import {
  formatBudget,
  getProcessVoiceSessionBudget,
  startMultiClientVoiceServer,
} from '@node-webrtc-rust/helpers'

import { resolveVoiceConfig } from '../../voice-agent-local-sherpa/src/resolve-voice-config.js'
import { isVoiceDebugEnabled } from '@node-webrtc-rust/sdk/voice'

import { voiceHandler } from './voice-handler.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const SHARED_DIR = join(__dirname, '../../shared')
const PORT = Number(process.env.PORT ?? 3004)
const ROOM = process.env.VOICE_ROOM ?? 'sherpa-multi'
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

async function readJsonBody(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = []
  for await (const chunk of req) {
    chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk)
  }
  const raw = Buffer.concat(chunks).toString('utf8').trim()
  if (!raw) return {}
  return JSON.parse(raw) as unknown
}

async function serveStatic(
  req: IncomingMessage,
  res: ServerResponse,
  broadcastSpeak?: (text: string) => Promise<string[]>,
): Promise<void> {
  const pathname = req.url?.split('?')[0] ?? '/'

  if (req.method === 'POST' && pathname === '/api/broadcast-speak' && broadcastSpeak) {
    try {
      const body = (await readJsonBody(req)) as { text?: string }
      const text = typeof body.text === 'string' ? body.text : ''
      const peerIds = await broadcastSpeak(text)
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ ok: true, text: text.trim(), peerIds, count: peerIds.length }))
      return
    } catch (error: unknown) {
      res.writeHead(400, { 'Content-Type': 'application/json' })
      res.end(
        JSON.stringify({
          ok: false,
          error: error instanceof Error ? error.message : 'broadcast failed',
        }),
      )
      return
    }
  }

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
  const sessionBudget = getProcessVoiceSessionBudget()

  const server = await startMultiClientVoiceServer({
    port: PORT,
    room: ROOM,
    voiceConfig,
    iceServers: ICE_SERVERS,
    sessionBudget,
    voiceHandler,
    serveHttp: (req, res) => serveStatic(req, res, server.broadcastSpeak),
    hostOptions: {
      log: (message) => console.log(message),
    },
  })

  console.log(`Local Sherpa multi-client demo at ${server.httpUrl}`)
  console.log(`Room: ${ROOM} (use the same room in every tab)`)
  console.log(`Signaling: ${server.signalingUrl}`)
  console.log(`Capacity: GET ${server.httpUrl}/api/capacity`)
  console.log(`Broadcast TTS: POST ${server.httpUrl}/api/broadcast-speak  body: {"text":"..."}`)
  console.log(`Voice pipeline: ${voiceLabel}`)
  console.log(`STT=local-sherpa (${language})  TTS=local-sherpa`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`Session budget: ${formatBudget(server.budget)}`)
  console.log('')
  console.log('Open three tabs → Connect → speak: TTS replies to that tab only.')
  console.log(
    'Use Speak (per tab) or POST /api/broadcast-speak / page “Speak to all” for all tabs.',
  )
  console.log('Edit src/voice-handler.ts to customize onSpeechEvent / onSpeakRequest.')
  console.log('Each tab = one client-* peer = one VoiceAgent; Sherpa models are shared in Rust.')
  if (sessionBudget.max > 0) {
    console.log(`Set VOICE_MAX_CONCURRENT_SESSIONS=${sessionBudget.max} — extra tabs are rejected.`)
  } else {
    console.log(
      'Tip: VOICE_MAX_CONCURRENT_SESSIONS=2 npm run start:cap-2 --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client to test rejection.',
    )
  }

  if (isVoiceDebugEnabled()) {
    console.error('[voice-debug] VOICE_DEBUG=1 — stderr pipeline logs enabled')
  }

  const shutdown = async () => {
    await server.close()
    process.exit(0)
  }

  process.on('SIGINT', () => void shutdown())
  process.on('SIGTERM', () => void shutdown())
}

void main().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
