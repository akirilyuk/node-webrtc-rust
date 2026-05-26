import { EventEmitter } from 'events'

import WebSocket from 'ws'

import type {
  AnswerEvent,
  IceCandidateEvent,
  OfferEvent,
  SignalingClientOptions,
  SignalingMessage,
} from './types'

export class SignalingClient extends EventEmitter {
  private ws: WebSocket | null = null
  readonly peerId: string
  readonly room: string
  private readonly url: string

  constructor(options: SignalingClientOptions) {
    super()
    this.url = options.url
    this.room = options.room
    this.peerId = options.peerId ?? randomPeerId()
  }

  connect(): Promise<void> {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      return Promise.resolve()
    }

    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(this.url)
      this.ws.once('open', () => {
        this.send({ type: 'join', room: this.room, peerId: this.peerId })
        this.emit('connected')
        resolve()
      })
      this.ws.once('error', (error) => {
        this.emit('error', error)
        reject(error)
      })
      this.ws.on('message', (raw) => this.handleMessage(raw.toString()))
      this.ws.on('close', () => this.emit('disconnected'))
    })
  }

  disconnect(): void {
    this.ws?.close()
    this.ws = null
  }

  sendOffer(targetPeerId: string, sdp: OfferEvent['sdp']): void {
    this.send({
      type: 'offer',
      room: this.room,
      peerId: this.peerId,
      targetPeerId,
      sdp,
    })
  }

  sendAnswer(targetPeerId: string, sdp: AnswerEvent['sdp']): void {
    this.send({
      type: 'answer',
      room: this.room,
      peerId: this.peerId,
      targetPeerId,
      sdp,
    })
  }

  sendIceCandidate(targetPeerId: string, candidate: IceCandidateEvent['candidate']): void {
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
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message))
    }
  }
}

function randomPeerId(): string {
  return `peer-${Math.random().toString(36).slice(2, 10)}`
}

export type { SignalingClientOptions } from './types'
