/**
 * Reusable pattern: one signaling room, many browser clients, one VoiceAgent each.
 *
 * Use this when several tabs (or devices) join the **same room** and each needs its own
 * WebRTC connection + {@link VoiceAgent} — for example three clients in `demo-room`.
 *
 * For **many independent calls** (different room ids per customer), use {@link SessionPod}.
 */

import { createServer, type IncomingMessage, type ServerResponse } from 'http'

import { SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'
import type { VoiceAgentConfig } from '@node-webrtc-rust/sdk/voice'

import {
  getProcessVoiceSessionBudget,
  type VoiceSessionBudget,
  type VoiceSessionBudgetSnapshot,
} from './voice-session-budget.js'
import {
  VOICE_AGENT_SERVER_PEER_ID,
  VoiceAgentSessionHost,
  type VoiceAgentSessionHostOptions,
} from './voice-agent-session-host.js'
import type { VoiceSessionHandler } from './voice-session-handler.js'

export interface IceServerConfig {
  urls: string | string[]
}

export interface MultiClientVoiceServerOptions {
  /** Signaling + HTTP port (default 3002). */
  port?: number
  /** WebSocket path (default `/ws`). */
  signalingPath?: string
  /** Signaling room id (default `demo`). */
  room?: string
  /** Shared VoiceAgent pipeline config (STT/TTS/VAD). */
  voiceConfig: VoiceAgentConfig
  iceServers: IceServerConfig[]
  /**
   * Process-wide connection cap. Defaults to {@link getProcessVoiceSessionBudget}
   * (`VOICE_MAX_CONCURRENT_SESSIONS`).
   */
  sessionBudget?: VoiceSessionBudget
  /** Passed through to {@link VoiceAgentSessionHost}. */
  hostOptions?: Pick<VoiceAgentSessionHostOptions, 'clientPeerIdPrefix' | 'log' | 'voiceHandler'>
  /**
   * Shorthand for `hostOptions.voiceHandler` — your STT/TTS app logic.
   * See `examples/voice-agent-local-sherpa-multi-client/src/voice-handler.ts`.
   */
  voiceHandler?: VoiceSessionHandler
  /** Serve static pages and other routes after the built-in `/api/capacity` handler. */
  serveHttp?: (req: IncomingMessage, res: ServerResponse) => Promise<void>
}

export interface MultiClientVoiceServerHandle {
  port: number
  room: string
  signalingPath: string
  signalingUrl: string
  httpUrl: string
  budget: VoiceSessionBudgetSnapshot
  close: () => Promise<void>
}

/**
 * Starts HTTP + WebSocket signaling and a {@link VoiceAgentSessionHost} for one room.
 *
 * **Three clients in one room:** open three browser tabs, same room string, each connects
 * as `client-<unique>`. The host creates three peer connections and three VoiceAgents.
 * Sherpa STT/TTS weights are shared in the native layer; each agent keeps its own stream state.
 */
export async function startMultiClientVoiceServer(
  options: MultiClientVoiceServerOptions,
): Promise<MultiClientVoiceServerHandle> {
  const port = options.port ?? 3002
  const signalingPath = options.signalingPath ?? '/ws'
  const room = options.room ?? 'demo'
  const sessionBudget = options.sessionBudget ?? getProcessVoiceSessionBudget()

  const httpServer = createServer((req, res) => {
    void handleHttp(req, res, sessionBudget, options.serveHttp)
  })

  const signalingServer = new SignalingServer({ server: httpServer, path: signalingPath })
  await signalingServer.listen(port)

  const serverSignaling = new SignalingClient({
    url: `ws://127.0.0.1:${port}${signalingPath}`,
    room,
    peerId: VOICE_AGENT_SERVER_PEER_ID,
  })
  await serverSignaling.connect()

  const host = new VoiceAgentSessionHost(serverSignaling, options.iceServers, {
    voiceConfig: options.voiceConfig,
    sessionBudget,
    ...options.hostOptions,
    voiceHandler: options.voiceHandler ?? options.hostOptions?.voiceHandler,
  })

  const log = options.hostOptions?.log ?? ((message: string) => console.log(message))
  log(`[voice-server] room=${room} port=${port} budget=${formatBudget(sessionBudget.snapshot())}`)

  return {
    port,
    room,
    signalingPath,
    signalingUrl: `ws://127.0.0.1:${port}${signalingPath}`,
    httpUrl: `http://127.0.0.1:${port}`,
    get budget() {
      return sessionBudget.snapshot()
    },
    close: async () => {
      await host?.close()
      serverSignaling?.disconnect()
      await signalingServer?.close()
    },
  }
}

async function handleHttp(
  req: IncomingMessage,
  res: ServerResponse,
  budget: VoiceSessionBudget,
  serveHttp: MultiClientVoiceServerOptions['serveHttp'],
): Promise<void> {
  const pathname = req.url?.split('?')[0] ?? '/'

  if (req.method === 'GET' && pathname === '/api/capacity') {
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ voice: budget.snapshot() }))
    return
  }

  if (serveHttp) {
    await serveHttp(req, res)
    return
  }

  res.writeHead(404)
  res.end('Not found')
}

export function formatBudget(snapshot: VoiceSessionBudgetSnapshot): string {
  if (snapshot.max === 0) {
    return `unlimited active=${snapshot.active} rejected=${snapshot.rejectedTotal}`
  }
  return `${snapshot.active}/${snapshot.max} available=${snapshot.available} rejected=${snapshot.rejectedTotal}`
}
