import { EventEmitter } from 'events'
import { createServer, type Server as HttpServer } from 'http'

import { WebSocket, WebSocketServer } from 'ws'

import { debugEvent, debugFn } from './debug'
import type { SignalingMessage, SignalingServerOptions } from './types'

interface Peer {
  id: string
  room: string
  socket: WebSocket
}

/**
 * Minimal room-based WebSocket signaling server for SDP and ICE relay.
 *
 * Peers join a named room, receive `peer-joined` notifications, and exchange
 * targeted `offer`, `answer`, and `ice-candidate` messages.
 */
export class SignalingServer extends EventEmitter {
  private readonly wss: WebSocketServer
  private readonly attachedServer?: HttpServer
  private readonly rooms = new Map<string, Map<string, Peer>>()
  private httpServer: HttpServer | null = null
  private listeningPort = 0
  private readonly pingIntervalMs: number
  private pingTimer: NodeJS.Timeout | null = null

  /** @param options - Optional HTTP server attachment or WebSocket path. */
  constructor(options: SignalingServerOptions = {}) {
    super()
    this.attachedServer = options.server
    this.pingIntervalMs = options.pingIntervalMs ?? 5_000
    this.wss = options.server
      ? new WebSocketServer({ server: options.server, path: options.path })
      : new WebSocketServer({ noServer: true })
  }

  /** Port the HTTP server is listening on after {@link listen}. */
  get port(): number {
    return this.listeningPort
  }

  /**
   * Starts the HTTP server and WebSocket upgrade handler.
   * @param port - Port to bind; use `0` for an ephemeral port.
   */
  listen(port = 8080): Promise<void> {
    debugFn('signaling::SignalingServer', 'listen', `port=${port}`)
    if (this.httpServer) {
      return Promise.resolve()
    }

    if (this.attachedServer) {
      const httpServer = this.attachedServer
      this.httpServer = httpServer
      return new Promise((resolve, reject) => {
        this.wss.on('connection', (socket) => this.handleConnection(socket))
        this.startPingTimer()

        const onListening = () => {
          const address = httpServer.address()
          this.listeningPort = typeof address === 'object' && address ? address.port : port
          resolve()
        }

        if (httpServer.listening) {
          onListening()
          return
        }

        httpServer.listen(port, onListening)
        httpServer.on('error', reject)
      })
    }

    return new Promise((resolve, reject) => {
      this.httpServer = createServer()
      this.httpServer.on('upgrade', (request, socket, head) => {
        this.wss.handleUpgrade(request, socket, head, (ws) => {
          this.wss.emit('connection', ws, request)
        })
      })
      this.httpServer.listen(port, () => {
        const address = this.httpServer?.address()
        this.listeningPort = typeof address === 'object' && address ? address.port : port
        this.wss.on('connection', (socket) => this.handleConnection(socket))
        this.startPingTimer()
        resolve()
      })
      this.httpServer.on('error', reject)
    })
  }

  /** Closes all peer sockets and the underlying HTTP/WebSocket servers. */
  close(): Promise<void> {
    debugFn('signaling::SignalingServer', 'close')
    this.stopPingTimer()
    for (const room of this.rooms.values()) {
      for (const peer of room.values()) {
        peer.socket.close()
      }
    }
    this.rooms.clear()

    return new Promise((resolve, reject) => {
      this.wss.close((error) => {
        if (error) {
          reject(error)
          return
        }
        if (this.httpServer) {
          this.httpServer.close(() => resolve())
          this.httpServer = null
        } else {
          resolve()
        }
      })
    })
  }

  private handleConnection(socket: WebSocket): void {
    debugEvent('signaling::SignalingServer', 'connection')
    let peer: Peer | null = null

    const trackedSocket = socket as WebSocket & { isAlive?: boolean }
    trackedSocket.isAlive = true
    socket.on('pong', () => {
      trackedSocket.isAlive = true
    })

    socket.on('message', (raw) => {
      let message: SignalingMessage
      try {
        message = JSON.parse(raw.toString()) as SignalingMessage
      } catch {
        return
      }

      debugEvent('signaling::SignalingServer', 'message', `type=${message.type}`)

      switch (message.type) {
        case 'join': {
          peer = { id: message.peerId, room: message.room, socket }
          let room = this.rooms.get(message.room)
          if (!room) {
            room = new Map()
            this.rooms.set(message.room, room)
          }
          const peersBeforeJoin = room.size
          const existing = room.get(message.peerId)
          if (existing && existing.socket !== socket) {
            // Replace stale socket (tab refresh / reconnect with same peerId).
            existing.socket.close()
            room.delete(message.peerId)
          }
          let peerJoinedNotifies = 0
          for (const other of room.values()) {
            if (other.id !== message.peerId) {
              send(other.socket, { type: 'peer-joined', peerId: message.peerId })
              send(socket, { type: 'peer-joined', peerId: other.id })
              peerJoinedNotifies += 2
            }
          }
          room.set(message.peerId, peer)
          if (message.peerId.startsWith('client-')) {
            logSignalingRoom('join', {
              room: message.room,
              peerId: message.peerId,
              peersBeforeJoin,
              peersAfterJoin: room.size,
              peerJoinedNotifies,
              replacedStaleSocket: existing != null && existing.socket !== socket,
            })
          }
          break
        }
        case 'offer':
        case 'answer':
        case 'ice-candidate':
          this.forward(message.room, message.targetPeerId, message)
          break
        default:
          break
      }
    })

    socket.on('close', () => {
      if (!peer) return
      debugEvent('signaling::SignalingServer', 'disconnect', `peerId=${peer.id}`)
      if (peer.id.startsWith('client-')) {
        logSignalingRoom('disconnect', { room: peer.room, peerId: peer.id })
      }
      const room = this.rooms.get(peer.room)
      room?.delete(peer.id)
      if (room && room.size === 0) {
        this.rooms.delete(peer.room)
      }
      this.broadcast(peer.room, { type: 'peer-left', peerId: peer.id }, peer.id)
    })
  }

  private startPingTimer(): void {
    if (this.pingIntervalMs <= 0 || this.pingTimer) return
    this.pingTimer = setInterval(() => {
      for (const client of this.wss.clients) {
        if (client.readyState !== WebSocket.OPEN) continue
        const trackedClient = client as WebSocket & { isAlive?: boolean }
        if (trackedClient.isAlive === false) {
          trackedClient.terminate()
          continue
        }
        trackedClient.isAlive = false
        trackedClient.ping()
      }
    }, this.pingIntervalMs)
  }

  private stopPingTimer(): void {
    if (!this.pingTimer) return
    clearInterval(this.pingTimer)
    this.pingTimer = null
  }

  private forward(roomId: string, targetPeerId: string, message: SignalingMessage): void {
    debugFn(
      'signaling::SignalingServer',
      'forward',
      `room=${roomId}, targetPeerId=${targetPeerId}, type=${message.type}`,
    )
    const peer = this.rooms.get(roomId)?.get(targetPeerId)
    if (peer) {
      send(peer.socket, message)
    }
  }

  private broadcast(roomId: string, message: SignalingMessage, excludePeerId?: string): void {
    const room = this.rooms.get(roomId)
    if (!room) return
    for (const [peerId, peer] of room) {
      if (peerId !== excludePeerId) {
        send(peer.socket, message)
      }
    }
  }
}

function send(socket: WebSocket, message: SignalingMessage): void {
  debugFn('signaling::SignalingServer', 'send', `type=${message.type}`)
  if (socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(message))
  }
}

/** Always-on room events for Loki triage (join / disconnect). */
function logSignalingRoom(event: string, detail: Record<string, unknown>): void {
  console.log(`[signaling-room] ${event} ${JSON.stringify(detail)}`)
}
