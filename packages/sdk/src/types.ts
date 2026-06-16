import type { RTCDataChannel } from './RTCDataChannel'
import type { MediaStream } from './MediaStream'
import type { MediaStreamTrack } from './MediaStreamTrack'
import type { RTCIceCandidate } from './RTCIceCandidate'

/** SDP session description type per the WebRTC specification. */
export type RTCSdpType = 'offer' | 'answer' | 'pranswer' | 'rollback'

/** Overall peer connection lifecycle state. */
export type RTCPeerConnectionState =
  | 'new'
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'failed'
  | 'closed'

/** ICE transport connectivity state. */
export type RTCIceConnectionState =
  | 'new'
  | 'checking'
  | 'connected'
  | 'completed'
  | 'disconnected'
  | 'failed'
  | 'closed'

/** ICE candidate gathering progress. */
export type RTCIceGatheringState = 'new' | 'gathering' | 'complete'

/** SDP offer/answer negotiation state. */
export type RTCSignalingState =
  | 'stable'
  | 'have-local-offer'
  | 'have-remote-offer'
  | 'have-local-pranswer'
  | 'have-remote-pranswer'
  | 'closed'

/** Data channel connection state. */
export type RTCDataChannelState = 'connecting' | 'open' | 'closing' | 'closed'

/** Media track kind. */
export type TrackKind = 'audio' | 'video'

/** STUN or TURN server entry in {@link RTCConfiguration}. */
export interface RTCIceServer {
  /** One or more `stun:` or `turn:` URLs. */
  urls: string | string[]
  /** TURN username (required for authenticated TURN servers). */
  username?: string
  /** TURN password or time-limited credential. */
  credential?: string
  /** How the credential should be interpreted; defaults to `password`. */
  credentialType?: 'password' | 'oauth'
}

/** Unified Plan transceiver direction. */
export type RTCRtpTransceiverDirection = 'sendrecv' | 'sendonly' | 'recvonly' | 'inactive'

/** Options for {@link RTCPeerConnection.addTransceiver}. */
export interface RTCRtpTransceiverInit {
  direction?: RTCRtpTransceiverDirection
}

/** Peer connection ICE and transport settings. */
export interface RTCConfiguration {
  iceServers?: RTCIceServer[]
  /** When `relay`, only TURN relay candidates are used. */
  iceTransportPolicy?: 'all' | 'relay'
  /**
   * When `true`, emit `[webrtc-debug]` logs for SDK and native calls.
   * Overrides `WEBRTC_DEBUG` when set; also set `WEBRTC_DEBUG=1` for process-wide logging.
   */
  debug?: boolean
}

/** Options passed to {@link RTCPeerConnection.createOffer}. */
export interface RTCOfferOptions {
  offerToReceiveAudio?: boolean
  offerToReceiveVideo?: boolean
  iceRestart?: boolean
  voiceActivityDetection?: boolean
}

/** Options passed to {@link RTCPeerConnection.createAnswer}. */
export interface RTCAnswerOptions {
  voiceActivityDetection?: boolean
}

/** Plain-object ICE candidate for signaling transport. */
export interface RTCIceCandidateInit {
  candidate?: string
  sdpMid?: string | null
  sdpMLineIndex?: number | null
  usernameFragment?: string | null
}

/** Options for {@link RTCPeerConnection.createDataChannel}. */
export interface RTCDataChannelInit {
  ordered?: boolean
  maxPacketLifeTime?: number
  maxRetransmits?: number
  protocol?: string
  negotiated?: number
}

/** Event payload for {@link RTCPeerConnection.onicecandidate}. */
export interface RTCPeerConnectionIceEvent {
  /** Local candidate, or `null` when gathering has finished. */
  candidate: RTCIceCandidate | null
}

/** Error event emitted by data channels. */
export interface RTCErrorEvent {
  type: 'error'
  message: string
}

/**
 * Event payload for {@link RTCPeerConnection.ontrack}.
 *
 * Emitted only after the remote sender writes the first audio sample via
 * {@link LocalAudioTrack.writeSample}; track negotiation alone is insufficient.
 */
export interface RTCTrackEvent {
  track: MediaStreamTrack
  streams: MediaStream[]
}

/** Event payload for {@link RTCPeerConnection.ondatachannel}. */
export interface RTCDataChannelEvent {
  channel: RTCDataChannel
}

/** Incoming data channel message. */
export interface MessageEvent<T = string | Buffer | ArrayBuffer> {
  data: T
}

/** WebRTC statistics entry (W3C `RTCStats` dictionary subset). */
export type RTCStats = Record<string, unknown>

/** Map of stat id → stat object, matching browser `RTCStatsReport`. */
export type RTCStatsReport = Map<string, RTCStats>

/** Plain-object session description for signaling transport. */
export interface RTCSessionDescriptionInit {
  type: RTCSdpType
  sdp: string
}
