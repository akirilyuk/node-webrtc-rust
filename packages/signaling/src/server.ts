import { EventEmitter } from 'events'
import { createServer, type Server as HttpServer } from 'http'

import { WebSocket, WebSocketServer } from 'ws'

import type { SignalingMessage, SignalingServerOptions } from './types'

interface Peer {
  id: string
  room: string
  socket: WebSocket
}

export class SignalingServer extends EventEmitter {
  private readonly wss: WebSocketServer
  private readonly rooms = new Map<string, Map<string, Peer>>()
  private httpServer: HttpServer | null = null
  private listeningPort = 0

  constructor(options: SignalingServerOptions = {}) {
    super()
    this.wss = options.server
      ? new WebSocketServer({ server: options.server, path: options.path })
      : new WebSocketServer({ noServer: true })
  }

  get port(): number {
    return this.listeningPort
  }

  listen(port = 8080): Promise<void> {
    if (this.httpServer) {
      return Promise.resolve()
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
        resolve()
      })
      this.httpServer.on('error', reject)
    })
  }

  close(): Promise<void> {
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
    let peer: Peer | null = null

    socket.on('message', (raw) => {
      let message: SignalingMessage
      try {
        message = JSON.parse(raw.toString()) as SignalingMessage
      } catch {
        return
      }

      switch (message.type) {
        case 'join': {
          peer = { id: message.peerId, room: message.room, socket }
          let room = this.rooms.get(message.room)
          if (!room) {
            room = new Map()
            this.rooms.set(message.room, room)
          }
          for (const other of room.values()) {
            if (other.id !== message.peerId) {
              send(other.socket, { type: 'peer-joined', peerId: message.peerId })
              send(socket, { type: 'peer-joined', peerId: other.id })
            }
          }
          room.set(message.peerId, peer)
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
      const room = this.rooms.get(peer.room)
      room?.delete(peer.id)
      if (room && room.size === 0) {
        this.rooms.delete(peer.room)
      }
      this.broadcast(peer.room, { type: 'peer-left', peerId: peer.id }, peer.id)
    })
  }

  private forward(roomId: string, targetPeerId: string, message: SignalingMessage): void {
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
  if (socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(message))
  }
}
