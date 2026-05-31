/**
 * Minimal room-based WebSocket signaling relay + conference bridge.
 *
 * Implements the same wire protocol as `@node-webrtc-rust/signaling` without
 * importing that package. Browser clients exchange `join`, `offer`, `answer`,
 * and `ice-candidate` messages; the server forwards them to peers and translates
 * conference-server traffic into native {@link ConferenceRoom.handleSignalingMessage} DTOs.
 */

import type { Server as HttpServer } from 'http'

import type { RTCIceCandidateInit, RTCSessionDescriptionInit } from '@node-webrtc-rust/sdk'
import type {
  ConferenceRoom,
  ConferenceServer,
  RoomOptions,
} from '@node-webrtc-rust/sdk/conference'
import type { ConferenceSignalingResponse } from '@node-webrtc-rust/sdk/conference'
import { WebSocket, WebSocketServer } from 'ws'

/** Wire messages exchanged over the signaling WebSocket (matches `@node-webrtc-rust/signaling`). */
export type SignalingMessage =
  | { type: 'join'; room: string; peerId: string }
  | {
      type: 'offer'
      room: string
      peerId: string
      targetPeerId: string
      sdp: RTCSessionDescriptionInit
    }
  | {
      type: 'answer'
      room: string
      peerId: string
      targetPeerId: string
      sdp: RTCSessionDescriptionInit
    }
  | {
      type: 'ice-candidate'
      room: string
      peerId: string
      targetPeerId: string
      candidate: RTCIceCandidateInit
    }
  | { type: 'peer-joined'; peerId: string }
  | { type: 'peer-left'; peerId: string }

interface Peer {
  id: string
  room: string
  socket: WebSocket
}

export interface ManualSignalingOptions {
  /** WebSocket path on the attached HTTP server. */
  path: string
  /** Synthetic peer id browsers use as the conference server WebRTC endpoint. */
  serverPeerId?: string
  /** Options used when a room is auto-created on first browser join. */
  defaultRoomOptions?: RoomOptions
  /** Called when conference bridge handling throws. */
  onError?: (roomId: string, message: string) => void
}

const DEFAULT_SERVER_PEER_ID = 'conference-server'

function send(socket: WebSocket, message: SignalingMessage): void {
  if (socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(message))
  }
}

function parseResponses(json: string): ConferenceSignalingResponse[] {
  if (!json || json === '[]') {
    return []
  }

  const parsed: unknown = JSON.parse(json)
  if (!Array.isArray(parsed)) {
    return [parsed as ConferenceSignalingResponse]
  }
  return parsed as ConferenceSignalingResponse[]
}

/**
 * Hand-rolled signaling server that relays SDP/ICE between browser tabs and
 * wires each room to the native conference engine.
 */
export class ManualSignalingServer {
  private readonly wss: WebSocketServer
  private readonly serverPeerId: string
  private readonly defaultRoomOptions?: RoomOptions
  private readonly onError?: (roomId: string, message: string) => void
  private readonly rooms = new Map<string, Map<string, Peer>>()
  private conferenceServer?: ConferenceServer

  constructor(httpServer: HttpServer, options: ManualSignalingOptions) {
    this.serverPeerId = options.serverPeerId ?? DEFAULT_SERVER_PEER_ID
    this.defaultRoomOptions = options.defaultRoomOptions
    this.onError = options.onError
    this.wss = new WebSocketServer({ server: httpServer, path: options.path })
    this.wss.on('connection', (socket) => this.handleConnection(socket))
  }

  /** Registers the conference server whose rooms should receive bridge wiring. */
  attachConference(server: ConferenceServer): void {
    this.conferenceServer = server
  }

  /** Returns an existing conference room, creating one on first join when configured. */
  async getOrCreateConferenceRoom(roomId: string): Promise<ConferenceRoom | undefined> {
    if (!this.conferenceServer) {
      return undefined
    }

    const existing = await this.conferenceServer.getRoom(roomId)
    if (existing) {
      return existing
    }

    if (!this.defaultRoomOptions) {
      return undefined
    }

    return this.conferenceServer.createRoom(roomId, this.defaultRoomOptions)
  }

  /** Closes all peer sockets and the WebSocket server. */
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
        resolve()
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
        case 'join':
          peer = { id: message.peerId, room: message.room, socket }
          this.addPeer(peer)
          void this.onBrowserJoined(message.room, message.peerId)
          break
        case 'offer':
        case 'answer':
        case 'ice-candidate':
          if (message.targetPeerId === this.serverPeerId) {
            void this.routeToConference(message.room, message)
          } else {
            this.forward(message.room, message.targetPeerId, message)
          }
          break
        default:
          break
      }
    })

    socket.on('close', () => {
      if (!peer) {
        return
      }
      this.removePeer(peer)
      void this.onBrowserLeft(peer.room, peer.id)
    })
  }

  private addPeer(peer: Peer): void {
    let room = this.rooms.get(peer.room)
    if (!room) {
      room = new Map()
      this.rooms.set(peer.room, room)
    }

    for (const other of room.values()) {
      if (other.id !== peer.id) {
        send(other.socket, { type: 'peer-joined', peerId: peer.id })
        send(peer.socket, { type: 'peer-joined', peerId: other.id })
      }
    }

    room.set(peer.id, peer)
  }

  private removePeer(peer: Peer): void {
    const room = this.rooms.get(peer.room)
    room?.delete(peer.id)
    if (room && room.size === 0) {
      this.rooms.delete(peer.room)
    }
    this.broadcast(peer.room, { type: 'peer-left', peerId: peer.id }, peer.id)
  }

  private forward(roomId: string, targetPeerId: string, message: SignalingMessage): void {
    const target = this.rooms.get(roomId)?.get(targetPeerId)
    if (target) {
      send(target.socket, message)
    }
  }

  private broadcast(roomId: string, message: SignalingMessage, excludePeerId?: string): void {
    const room = this.rooms.get(roomId)
    if (!room) {
      return
    }

    for (const [peerId, peer] of room) {
      if (peerId !== excludePeerId) {
        send(peer.socket, message)
      }
    }
  }

  private async onBrowserJoined(roomId: string, peerId: string): Promise<void> {
    if (!this.conferenceServer) {
      return
    }

    try {
      const room = await this.getOrCreateConferenceRoom(roomId)
      if (!room) {
        return
      }
      const responses = await room.handleSignalingMessage(
        JSON.stringify({
          type: 'join',
          participantId: peerId,
          roomId,
        }),
      )
      await this.forwardConferenceResponses(roomId, peerId, responses)
    } catch (error) {
      this.reportError(roomId, error)
    }
  }

  private async onBrowserLeft(roomId: string, peerId: string): Promise<void> {
    const room = await this.conferenceServer?.getRoom(roomId)
    if (!room) {
      return
    }

    try {
      await room.handleSignalingMessage(
        JSON.stringify({
          type: 'leave',
          participantId: peerId,
        }),
      )
    } catch (error) {
      this.reportError(roomId, error)
    }
  }

  private async routeToConference(
    roomId: string,
    message: Extract<SignalingMessage, { type: 'offer' | 'answer' | 'ice-candidate' }>,
  ): Promise<void> {
    const room = await this.conferenceServer?.getRoom(roomId)
    if (!room) {
      return
    }

    try {
      if (message.type === 'offer') {
        const responses = await room.handleSignalingMessage(
          JSON.stringify({
            type: 'offer',
            participantId: message.peerId,
            sdp: message.sdp.sdp ?? '',
          }),
        )
        await this.forwardConferenceResponses(roomId, message.peerId, responses)
        return
      }

      if (message.type === 'answer') {
        await room.handleSignalingMessage(
          JSON.stringify({
            type: 'answer',
            participantId: message.peerId,
            sdp: message.sdp.sdp ?? '',
          }),
        )
        return
      }

      if (!message.candidate.candidate) {
        return
      }

      await room.handleSignalingMessage(
        JSON.stringify({
          type: 'iceCandidate',
          participantId: message.peerId,
          candidate: message.candidate.candidate,
          sdpMid: message.candidate.sdpMid,
          sdpMLineIndex: message.candidate.sdpMLineIndex,
        }),
      )
    } catch (error) {
      this.reportError(roomId, error)
    }
  }

  private async forwardConferenceResponses(
    roomId: string,
    targetPeerId: string,
    responsesJson: string,
  ): Promise<void> {
    const target = this.rooms.get(roomId)?.get(targetPeerId)
    if (!target) {
      return
    }

    for (const response of parseResponses(responsesJson)) {
      if (response.type === 'offer') {
        send(target.socket, {
          type: 'offer',
          room: roomId,
          peerId: this.serverPeerId,
          targetPeerId,
          sdp: { type: 'offer', sdp: response.sdp },
        })
      } else if (response.type === 'answer') {
        send(target.socket, {
          type: 'answer',
          room: roomId,
          peerId: this.serverPeerId,
          targetPeerId,
          sdp: { type: 'answer', sdp: response.sdp },
        })
      }
    }
  }

  private reportError(roomId: string, error: unknown): void {
    const message = error instanceof Error ? error.message : String(error)
    this.onError?.(roomId, message)
  }
}
