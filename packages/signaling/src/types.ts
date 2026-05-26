import type { Server as HttpServer } from 'http'

import type {
  RTCIceCandidateInit,
  RTCPeerConnection,
  RTCSessionDescriptionInit,
} from '@node-webrtc-rust/sdk'

import type { SignalingClient } from './client'

export interface SignalingServerOptions {
  port?: number
  server?: HttpServer
  path?: string
}

export interface SignalingClientOptions {
  url: string
  room: string
  peerId?: string
}

export interface AutoNegotiateOptions {
  pc: RTCPeerConnection
  signaling: SignalingClient
  polite: boolean
}

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

export type OfferEvent = { peerId: string; sdp: RTCSessionDescriptionInit }
export type AnswerEvent = { peerId: string; sdp: RTCSessionDescriptionInit }
export type IceCandidateEvent = { peerId: string; candidate: RTCIceCandidateInit }
