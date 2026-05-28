/**
 * Multi-session voice pod demo.
 *
 * One Node process = one pod with:
 * - a single HTTP + WebSocket signaling entry point
 * - many concurrent sessions (each sessionId is a signaling room)
 * - one VoiceAgent per WebRTC connection, cleaned up when the call ends
 *
 * Run:
 *   npm run start --workspace=@node-webrtc-rust/example-voice-agent-multi-session-pod
 *
 * Open http://localhost:3003 in multiple tabs — use a **different session ID per tab**
 * to simulate independent calls on the same pod. Watch the server log for session
 * create/destroy and per-connection VoiceAgent lifecycle.
 */

import { createServer, type IncomingMessage, type ServerResponse } from 'http'
import { readFile } from 'fs/promises'
import { dirname, extname, join } from 'path'
import { fileURLToPath } from 'url'

import { SignalingServer } from '@node-webrtc-rust/signaling'
import { SessionPod } from '@node-webrtc-rust/helpers'

import { resolveVoiceConfig } from '../../voice-agent-browser/src/resolve-voice-config.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const SHARED_DIR = join(__dirname, '../../shared')
const PORT = Number(process.env.PORT ?? 3003)
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
}

const { config: voiceConfig, label: voiceLabel } = resolveVoiceConfig()

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

async function readJsonBody(req: IncomingMessage): Promise<unknown> {
  let body = ''
  for await (const chunk of req) {
    body += chunk
  }
  return body ? JSON.parse(body) : {}
}

async function main(): Promise<void> {
  const runtime: { pod?: SessionPod } = {}

  const httpServer = createServer(async (req, res) => {
    const pod = runtime.pod
    if (!pod) {
      res.writeHead(503)
      res.end('Pod not ready')
      return
    }
    const pathname = req.url?.split('?')[0] ?? '/'

    if (req.method === 'POST' && pathname === '/api/sessions') {
      try {
        const payload = (await readJsonBody(req)) as { sessionId?: string }
        const sessionId = payload.sessionId?.trim()
        if (!sessionId) {
          res.writeHead(400, { 'Content-Type': 'application/json' })
          res.end(JSON.stringify({ error: 'sessionId is required' }))
          return
        }
        await pod.ensureSession(sessionId)
        res.writeHead(200, { 'Content-Type': 'application/json' })
        res.end(
          JSON.stringify({
            sessionId,
            activeSessions: pod.activeSessionCount,
            activeConnections: pod.activeConnectionCount,
          }),
        )
      } catch (error) {
        res.writeHead(500, { 'Content-Type': 'application/json' })
        res.end(JSON.stringify({ error: String(error) }))
      }
      return
    }

    if (req.method === 'GET' && pathname === '/api/sessions') {
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(
        JSON.stringify({
          activeSessions: pod.activeSessionCount,
          activeConnections: pod.activeConnectionCount,
          sessions: pod.listSessions(),
        }),
      )
      return
    }

    await serveStatic(req, res)
  })

  const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
  await signaling.listen(PORT)

  runtime.pod = new SessionPod(signaling, {
    signalingUrl: `ws://127.0.0.1:${PORT}/ws`,
    iceServers: ICE_SERVERS,
    voiceConfig,
    onSessionChange: ({ sessionId, action, activeSessions }) => {
      console.log(`[pod] ${action} session=${sessionId} activeSessions=${activeSessions}`)
    },
  })

  console.log(`Multi-session voice pod at http://localhost:${PORT}`)
  console.log(`Signaling entry point: ws://localhost:${PORT}/ws`)
  console.log(`Voice pipeline: ${voiceLabel}`)
  console.log('')
  console.log('Try: open multiple tabs with different session IDs (call-1, call-2, …).')
  console.log('Each tab = one session = one VoiceAgent while connected.')
  console.log('Disconnect a tab → agent stops and session slot is torn down when idle.')
  console.log(`Pod metrics: GET http://localhost:${PORT}/api/sessions`)

  const shutdown = async () => {
    await runtime.pod?.close()
    process.exit(0)
  }

  process.on('SIGINT', () => void shutdown())
  process.on('SIGTERM', () => void shutdown())
}

void main().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
