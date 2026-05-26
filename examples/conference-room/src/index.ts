import { createServer, type IncomingMessage, type ServerResponse } from 'http'
import { readFile } from 'fs/promises'
import { dirname, extname, join } from 'path'
import { fileURLToPath } from 'url'

import { ConferenceServer } from '@node-webrtc-rust/conference'
import type { MuteScope } from '@node-webrtc-rust/conference'
import { SignalingServer } from '@node-webrtc-rust/signaling'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const PORT = Number(process.env.PORT ?? 8080)
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
}

async function readJsonBody(req: IncomingMessage): Promise<unknown> {
  let body = ''
  for await (const chunk of req) {
    body += chunk
  }
  return body ? JSON.parse(body) : {}
}

function sendJson(res: ServerResponse, status: number, payload: unknown): void {
  res.writeHead(status, { 'Content-Type': 'application/json' })
  res.end(JSON.stringify(payload))
}

function roomFromApiPath(pathname: string, suffix: string): string | undefined {
  const prefix = '/api/rooms/'
  if (!pathname.startsWith(prefix) || !pathname.endsWith(suffix)) {
    return undefined
  }
  const room = decodeURIComponent(pathname.slice(prefix.length, pathname.length - suffix.length))
  return room || undefined
}

async function serveStatic(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const pathname = req.url?.split('?')[0] ?? '/'
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
  const conference = new ConferenceServer()
  conference.attachSignaling({ url: `ws://127.0.0.1:${PORT}/ws` })

  conference.on('mixing-enabled-changed', ({ roomId, enabled }) => {
    console.log(`[conference] mixing-enabled-changed room=${roomId} enabled=${enabled}`)
  })

  conference.on('participant-muted', ({ roomId, targetId, scope, listenerId }) => {
    console.log(
      `[conference] participant-muted room=${roomId} target=${targetId} scope=${scope}` +
        (listenerId ? ` listener=${listenerId}` : ''),
    )
  })

  conference.on('participant-kicked', ({ roomId, participantId, reason }) => {
    console.log(
      `[conference] participant-kicked room=${roomId} participant=${participantId}` +
        (reason ? ` reason=${reason}` : ''),
    )
  })

  conference.on('participant-joined', ({ roomId, participantId }) => {
    console.log(`[conference] participant-joined room=${roomId} participant=${participantId}`)
  })

  conference.on('participant-left', ({ roomId, participantId }) => {
    console.log(`[conference] participant-left room=${roomId} participant=${participantId}`)
  })

  conference.on('error', ({ roomId, message }) => {
    console.error(`[conference] error room=${roomId ?? 'unknown'} message=${message}`)
  })

  async function ensureRoom(roomId: string) {
    const existing = await conference.getRoom(roomId)
    if (existing) {
      return existing
    }
    return conference.createRoom(roomId, {
      maxParticipants: 16,
      iceServers: ICE_SERVERS,
    })
  }

  const httpServer = createServer(async (req, res) => {
    const pathname = req.url?.split('?')[0] ?? '/'

    if (req.method === 'POST' && pathname === '/api/rooms') {
      try {
        const body = (await readJsonBody(req)) as { room?: string }
        if (!body.room || typeof body.room !== 'string') {
          sendJson(res, 400, { error: 'room is required' })
          return
        }
        await ensureRoom(body.room)
        sendJson(res, 200, { room: body.room })
      } catch (error) {
        sendJson(res, 500, { error: String(error) })
      }
      return
    }

    const participantsRoom = roomFromApiPath(pathname, '/participants')
    if (req.method === 'GET' && participantsRoom) {
      try {
        const roomHandle = await conference.getRoom(participantsRoom)
        if (!roomHandle) {
          sendJson(res, 404, { error: 'room not found' })
          return
        }
        const participants = await roomHandle.listParticipants()
        const mixingEnabled = await roomHandle.isMixingEnabled()
        sendJson(res, 200, { participants, mixingEnabled })
      } catch (error) {
        sendJson(res, 500, { error: String(error) })
      }
      return
    }

    /*
     * Admin REST routes below are unauthenticated in this demo.
     * In production, require auth (session, JWT, or moderator role) before
     * forwarding mute, mixing, or kick calls to ConferenceRoom.
     */
    const muteRoom = roomFromApiPath(pathname, '/mute')
    if (req.method === 'POST' && muteRoom) {
      try {
        const body = (await readJsonBody(req)) as {
          targetId?: string
          scope?: MuteScope
          listenerId?: string
          muted?: boolean
        }
        if (!body.targetId || !body.scope) {
          sendJson(res, 400, { error: 'targetId and scope are required' })
          return
        }
        const roomHandle = await conference.getRoom(muteRoom)
        if (!roomHandle) {
          sendJson(res, 404, { error: 'room not found' })
          return
        }
        const mute = body.muted !== false
        if (mute) {
          await roomHandle.muteParticipant(body.targetId, {
            scope: body.scope,
            listenerId: body.listenerId,
          })
        } else {
          await roomHandle.unmuteParticipant(body.targetId, {
            scope: body.scope,
            listenerId: body.listenerId,
          })
        }
        sendJson(res, 200, { ok: true, muted: mute })
      } catch (error) {
        sendJson(res, 500, { error: String(error) })
      }
      return
    }

    const mixingRoom = roomFromApiPath(pathname, '/mixing')
    if (req.method === 'POST' && mixingRoom) {
      try {
        const body = (await readJsonBody(req)) as { enabled?: boolean }
        if (typeof body.enabled !== 'boolean') {
          sendJson(res, 400, { error: 'enabled boolean is required' })
          return
        }
        const roomHandle = await conference.getRoom(mixingRoom)
        if (!roomHandle) {
          sendJson(res, 404, { error: 'room not found' })
          return
        }
        await roomHandle.setMixingEnabled(body.enabled)
        sendJson(res, 200, { enabled: body.enabled })
      } catch (error) {
        sendJson(res, 500, { error: String(error) })
      }
      return
    }

    const kickRoom = roomFromApiPath(pathname, '/kick')
    if (req.method === 'POST' && kickRoom) {
      try {
        const body = (await readJsonBody(req)) as { participantId?: string; reason?: string }
        if (!body.participantId) {
          sendJson(res, 400, { error: 'participantId is required' })
          return
        }
        const roomHandle = await conference.getRoom(kickRoom)
        if (!roomHandle) {
          sendJson(res, 404, { error: 'room not found' })
          return
        }
        await roomHandle.kickParticipant(body.participantId, body.reason)
        sendJson(res, 200, { ok: true })
      } catch (error) {
        sendJson(res, 500, { error: String(error) })
      }
      return
    }

    await serveStatic(req, res)
  })

  const signaling = new SignalingServer({ server: httpServer, path: '/ws' })
  await signaling.listen(PORT)

  console.log(`Conference room example running at http://localhost:${PORT}`)
  console.log(`WebSocket signaling: ws://localhost:${PORT}/ws`)
  console.log(`Open multiple browser tabs, join the same room, and allow microphone access.`)
  console.log(`Set WEBRTC_DEBUG=1 to trace conference, signaling, and WebRTC calls.`)

  const shutdown = async () => {
    for (const roomId of await conference.listRooms()) {
      await conference.destroyRoom(roomId)
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
