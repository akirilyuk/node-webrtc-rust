/**
 * Voice+Data mix groups demo — positional client mixing in one Sherpa room.
 *
 * Demonstrates {@link VoiceAgentSessionHost} mix APIs:
 * - Two isolated groups (first three peers vs later peers)
 * - `setClientPose`, `setPositionalMixing`, `setTtsMixPlacement`
 *
 * Prerequisites: same Sherpa model paths as the main multi-client example (see README).
 *
 * Run:
 *   npm run start:mix-groups --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa-multi-client
 *
 * Open multiple tabs at http://localhost:3004 — peers are `client-*`.
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
import { isVoiceDebugEnabled } from '@node-webrtc-rust/sdk/voice'

import { freePort } from '../../shared/free-port.js'
import { resolveVoiceConfig } from '../../voice-agent-local-sherpa/src/resolve-voice-config.js'

import { voiceHandler } from './voice-handler.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PUBLIC_DIR = join(__dirname, '../public')
const SHARED_DIR = join(__dirname, '../../shared')
const PORT = Number(process.env.PORT ?? 3004)
const ROOM = process.env.VOICE_ROOM ?? 'sherpa-mix-groups'
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

let connectOrder = 0
let mixGroupsInitialized = false

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
  if (!process.env.WEBRTC_NAT_1TO1_IPS) {
    process.env.WEBRTC_NAT_1TO1_IPS = '127.0.0.1'
  }

  freePort(PORT, 'voice-agent-local-sherpa-multi-client')

  const sessionBudget = getProcessVoiceSessionBudget()

  const server = await startMultiClientVoiceServer({
    port: PORT,
    room: ROOM,
    voiceConfig,
    iceServers: ICE_SERVERS,
    sessionBudget,
    voiceHandler: {
      ...voiceHandler,
      onPeerConnected: async (ctx) => {
        await voiceHandler.onPeerConnected?.(ctx)
        connectOrder += 1
        const host = server.host
        if (!mixGroupsInitialized) {
          host.createMixGroup({ id: 'proximity', clientIds: [] })
          host.createMixGroup({ id: 'radio', clientIds: [] })
          mixGroupsInitialized = true
        }
        if (connectOrder <= 3) {
          host.addClientToMix('proximity', ctx.peerId)
        } else {
          host.addClientToMix('radio', ctx.peerId)
        }
        host.setPositionalMixing(true)
        host.setClientPose(ctx.peerId, {
          position: { x: connectOrder, y: 0, z: 0 },
          orientation: { x: 0, y: 0, z: 0, w: 1 },
        })
        if (connectOrder === 2) {
          host.setTtsMixPlacement('right')
        }
      },
    },
    serveHttp: (req, res) => serveStatic(req, res),
    hostOptions: {
      sessionMode: 'voice+data',
      log: (message) => {
        const ts = new Date().toISOString().slice(11, 23)
        console.log(`${ts} ${message}`)
      },
    },
  })

  console.log(`Mix groups demo at ${server.httpUrl}`)
  console.log(`Room: ${ROOM} — sessionMode voice+data (voice + sync data channel)`)
  console.log(`Voice pipeline: ${voiceLabel}`)
  console.log(`STT=local-sherpa (${language})  TTS=local-sherpa`)
  console.log(`SHERPA_STT_MODEL_PATH=${sttModelPath}`)
  console.log(`SHERPA_TTS_MODEL_PATH=${ttsModelPath}`)
  console.log(`Session budget: ${formatBudget(server.budget)}`)
  console.log('')
  console.log('Open tabs — first three peers share proximity group; later peers join radio.')
  console.log('Mix APIs: createMixGroup, addClientToMix, setClientPose, setPositionalMixing, …')

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
