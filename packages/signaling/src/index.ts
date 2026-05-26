/**
 * @packageDocumentation
 * WebSocket signaling helpers for node-webrtc-rust.
 *
 * Provides a minimal room-based SDP/ICE relay server, a client, and
 * {@link autoNegotiate} to wire {@link RTCPeerConnection} instances together.
 */
export { SignalingServer } from './server'
export { SignalingClient } from './client'
export { autoNegotiate } from './auto-negotiate'
export type {
  SignalingServerOptions,
  SignalingClientOptions,
  AutoNegotiateOptions,
  SignalingMessage,
  OfferEvent,
  AnswerEvent,
  IceCandidateEvent,
} from './types'

/** Signaling package version (informational). */
export const SIGNALING_VERSION = '0.1.0'
