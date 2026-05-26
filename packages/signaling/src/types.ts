import type { Server as HttpServer } from 'http'

import type {
  RTCIceCandidateInit,
  RTCPeerConnection,
  RTCSessionDescriptionInit,
} from '@node-webrtc-rust/sdk'

import type { SignalingClient } from './client'

/** Options for {@link SignalingServer}. */
export interface SignalingServerOptions {
  /** Preferred listen port when using the built-in HTTP server. */
  port?: number
  /** Attach to an existing HTTP server instead of creating one. */
  server?: HttpServer
  /** WebSocket path when using a custom HTTP server. */
  path?: string
}

/** Options for {@link SignalingClient}. */
export interface SignalingClientOptions {
  /** WebSocket URL, e.g. `ws://localhost:8080`. */
  url: string
  /** Room name; peers in the same room can exchange SDP/ICE. */
  room: string
  /** Stable peer id; a random id is generated when omitted. */
  peerId?: string
}

/** Arguments for {@link autoNegotiate}. */
export interface AutoNegotiateOptions {
  /** Local peer connection to negotiate. */
  pc: RTCPeerConnection
  /** Connected signaling client in the same room as the remote peer. */
  signaling: SignalingClient
  /**
   * When `true`, this peer yields on offer glare (waits for the remote offer).
   * When `false`, this peer creates the initial offer when a remote peer joins.
   */
  polite: boolean
}

/** Wire protocol messages exchanged over the signaling WebSocket. */
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

/** Payload for the `offer` client event. */
export type OfferEvent = { peerId: string; sdp: RTCSessionDescriptionInit }
/** Payload for the `answer` client event. */
export type AnswerEvent = { peerId: string; sdp: RTCSessionDescriptionInit }
/** Payload for the `ice-candidate` client event. */
export type IceCandidateEvent = { peerId: string; candidate: RTCIceCandidateInit }
