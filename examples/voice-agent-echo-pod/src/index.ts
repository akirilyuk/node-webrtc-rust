/**
 * Echo pod — SessionPod + inline echo {@link VoiceSessionHandler}.
 *
 * Same HTTP/signaling surface as runner M1 (`POST /api/sessions`, `GET /healthz`)
 * but **no runner process and no @voicethere/agent child**. Exercises only:
 *   SessionPod → VoiceAgentSessionHost → Sherpa STT/TTS
 *
 * Layer isolation (lowest voice stack before runner):
 *   npm run start --workspace=@node-webrtc-rust/example-voice-agent-echo-pod
 *   cd e2e && npm run test:local-direct-voice-reconnect
 *
 * Default port **8090** (runner M1 uses 8080).
 */

import { createServer, type IncomingMessage, type ServerResponse } from 'http'
import { readFile } from 'fs/promises'
import { dirname, extname, join } from 'path'
import { fileURLToPath } from 'url'

import { SignalingServer } from '@node-webrtc-rust/signaling'
import { SessionPod } from '@node-webrtc-rust/helpers'

import { echoVoiceHandler } from '../../shared/echo-voice-handler.js'
import { isSpeechEventLogEnabled } from '../../shared/speech-event-log.js'
import { DEMO_ICE_SERVERS } from '../../shared/webrtc-demo-helpers.js'
import { freePort } from '../../shared/free-port.js'
import { resolveVoiceConfig } from '../../voice-agent-local-sherpa/src/resolve-voice-config.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const SHARED_DIR = join(__dirname, '../../shared')
const PORT = Number(process.env.ECHO_POD_PORT ?? process.env.PORT ?? 8090)
const ICE_SERVERS = DEMO_ICE_SERVERS

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
}

const { config: voiceConfig, label: voiceLabel } = resolveVoiceConfig()

function parsePositiveInt(raw: string | undefined, fallback: number): number {
  const n = Number.parseInt(raw ?? '', 10)
  return Number.isFinite(n) && n >= 0 ? n : fallback
}

async function readJsonBody(req: IncomingMessage): Promise<unknown> {
  let body = ''
  for await (const chunk of req) {
    body += chunk
  }
  return body ? JSON.parse(body) : {}
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
  if (!process.env.WEBRTC_NAT_1TO1_IPS?.trim()) {
    process.env.WEBRTC_NAT_1TO1_IPS = '127.0.0.1'
  }

  const rejoinGraceMs = parsePositiveInt(process.env.SESSION_REJOIN_GRACE_MS, 90_000)
  const runtime: { pod?: SessionPod } = {}

  const httpServer = createServer(async (req, res) => {
    const pod = runtime.pod
    if (!pod) {
      res.writeHead(503)
      res.end('Pod not ready')
      return
    }
    const pathname = req.url?.split('?')[0] ?? '/'

    if (req.method === 'GET' && pathname === '/healthz') {
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ ok: true, layer: 'direct-echo-pod' }))
      return
    }

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

  freePort(PORT, 'voice-agent-echo-pod')

  const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
  await signaling.listen(PORT)

  runtime.pod = new SessionPod(signaling, {
    signalingUrl: `ws://127.0.0.1:${PORT}/ws`,
    iceServers: ICE_SERVERS,
    voiceConfig,
    voiceHandler: echoVoiceHandler,
    rejoinGraceMs,
    log: (message) => console.log(`[echo-pod] ${message.replace(/^\[pod\] /, '')}`),
    onSessionChange: ({ sessionId, action, activeSessions }) => {
      console.log(`[echo-pod] ${action} session=${sessionId} activeSessions=${activeSessions}`)
    },
  })

  console.log(`Echo pod (direct layer) at http://127.0.0.1:${PORT}`)
  console.log(`Signaling: ws://127.0.0.1:${PORT}/ws`)
  console.log(`Voice pipeline: ${voiceLabel}`)
  console.log(`Rejoin grace: ${rejoinGraceMs}ms`)
  if (isSpeechEventLogEnabled()) {
    console.error('[echo-pod] SPEECH_EVENT_LOG=1 — [speech] VoiceAgent events on stderr')
  }
  if (process.env.VOICE_DEBUG?.trim()) {
    console.error('[echo-pod] VOICE_DEBUG=1 — [voice-debug] Rust STT/VAD/TTS on stderr')
  }
  if (process.env.WEBRTC_DEBUG?.trim()) {
    console.error('[echo-pod] WEBRTC_DEBUG=1 — [webrtc-debug] SDK traces on stderr')
  }
  console.log('')
  console.log('E2E: cd e2e && npm run test:local-direct-voice-reconnect')
  console.log('Load: cd e2e && npm run test:local-direct-voice-load')

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
