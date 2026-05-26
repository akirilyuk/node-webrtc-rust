import { EventEmitter } from 'events'

import WebSocket from 'ws'

import { debugEvent, debugFn } from './debug'
import type {
  AnswerEvent,
  IceCandidateEvent,
  OfferEvent,
  SignalingClientOptions,
  SignalingMessage,
} from './types'

/**
 * WebSocket signaling client for a single room.
 *
 * Emits `peer-joined`, `offer`, `answer`, and `ice-candidate` events for use
 * with {@link autoNegotiate} or custom negotiation logic.
 */
export class SignalingClient extends EventEmitter {
  private ws: WebSocket | null = null
  /** This client's peer id (used as the `peerId` field in outbound messages). */
  readonly peerId: string
  /** Room this client joined. */
  readonly room: string
  private readonly url: string

  constructor(options: SignalingClientOptions) {
    super()
    this.url = options.url
    this.room = options.room
    this.peerId = options.peerId ?? randomPeerId()
  }

  /** Opens the WebSocket and sends a `join` message for {@link room}. */
  connect(): Promise<void> {
    debugFn('signaling::SignalingClient', 'connect', `url=${this.url}, room=${this.room}`)
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      return Promise.resolve()
    }

    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(this.url)
      this.ws.once('open', () => {
        debugEvent('signaling::SignalingClient', 'connected')
        this.send({ type: 'join', room: this.room, peerId: this.peerId })
        this.emit('connected')
        resolve()
      })
      this.ws.once('error', (error) => {
        this.emit('error', error)
        reject(error)
      })
      this.ws.on('message', (raw) => this.handleMessage(raw.toString()))
      this.ws.on('close', () => {
        debugEvent('signaling::SignalingClient', 'disconnected')
        this.emit('disconnected')
      })
    })
  }

  /** Closes the WebSocket connection. */
  disconnect(): void {
    debugFn('signaling::SignalingClient', 'disconnect')
    this.ws?.close()
    this.ws = null
  }

  /** Sends an SDP offer to a specific peer in the room. */
  sendOffer(targetPeerId: string, sdp: OfferEvent['sdp']): void {
    debugFn('signaling::SignalingClient', 'sendOffer', `targetPeerId=${targetPeerId}`)
    this.send({
      type: 'offer',
      room: this.room,
      peerId: this.peerId,
      targetPeerId,
      sdp,
    })
  }

  /** Sends an SDP answer to a specific peer in the room. */
  sendAnswer(targetPeerId: string, sdp: AnswerEvent['sdp']): void {
    debugFn('signaling::SignalingClient', 'sendAnswer', `targetPeerId=${targetPeerId}`)
    this.send({
      type: 'answer',
      room: this.room,
      peerId: this.peerId,
      targetPeerId,
      sdp,
    })
  }

  /** Sends a trickle ICE candidate to a specific peer in the room. */
  sendIceCandidate(targetPeerId: string, candidate: IceCandidateEvent['candidate']): void {
    debugFn('signaling::SignalingClient', 'sendIceCandidate', `targetPeerId=${targetPeerId}`)
    this.send({
      type: 'ice-candidate',
      room: this.room,
      peerId: this.peerId,
      targetPeerId,
      candidate,
    })
  }

  private handleMessage(raw: string): void {
    let message: SignalingMessage
    try {
      message = JSON.parse(raw) as SignalingMessage
    } catch {
      return
    }

    debugEvent('signaling::SignalingClient', 'message', `type=${message.type}`)

    switch (message.type) {
      case 'peer-joined':
        this.emit('peer-joined', message.peerId)
        break
      case 'peer-left':
        this.emit('peer-left', message.peerId)
        break
      case 'offer':
        this.emit('offer', { peerId: message.peerId, sdp: message.sdp } satisfies OfferEvent)
        break
      case 'answer':
        this.emit('answer', { peerId: message.peerId, sdp: message.sdp } satisfies AnswerEvent)
        break
      case 'ice-candidate':
        this.emit('ice-candidate', {
          peerId: message.peerId,
          candidate: message.candidate,
        } satisfies IceCandidateEvent)
        break
      default:
        break
    }
  }

  private send(message: SignalingMessage): void {
    debugFn('signaling::SignalingClient', 'send', `type=${message.type}`)
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message))
    }
  }
}

function randomPeerId(): string {
  return `peer-${Math.random().toString(36).slice(2, 10)}`
}

export type { SignalingClientOptions } from './types'
