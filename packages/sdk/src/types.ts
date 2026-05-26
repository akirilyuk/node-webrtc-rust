import type { RTCDataChannel } from './RTCDataChannel'
import type { MediaStream } from './MediaStream'
import type { MediaStreamTrack } from './MediaStreamTrack'
import type { RTCIceCandidate } from './RTCIceCandidate'

export type RTCSdpType = 'offer' | 'answer' | 'pranswer' | 'rollback'

export type RTCPeerConnectionState =
  | 'new'
  | 'connecting'
  | 'connected'
  | 'disconnected'
  | 'failed'
  | 'closed'

export type RTCIceConnectionState =
  | 'new'
  | 'checking'
  | 'connected'
  | 'completed'
  | 'disconnected'
  | 'failed'
  | 'closed'

export type RTCIceGatheringState = 'new' | 'gathering' | 'complete'

export type RTCSignalingState =
  | 'stable'
  | 'have-local-offer'
  | 'have-remote-offer'
  | 'have-local-pranswer'
  | 'have-remote-pranswer'
  | 'closed'

export type RTCDataChannelState = 'connecting' | 'open' | 'closing' | 'closed'

export type TrackKind = 'audio' | 'video'

export interface RTCIceServer {
  urls: string | string[]
  username?: string
  credential?: string
  credentialType?: 'password' | 'oauth'
}

export interface RTCConfiguration {
  iceServers?: RTCIceServer[]
  iceTransportPolicy?: 'all' | 'relay'
}

export interface RTCOfferOptions {
  offerToReceiveAudio?: boolean
  offerToReceiveVideo?: boolean
  iceRestart?: boolean
}

export interface RTCIceCandidateInit {
  candidate?: string
  sdpMid?: string | null
  sdpMLineIndex?: number | null
  usernameFragment?: string | null
}

export interface RTCDataChannelInit {
  ordered?: boolean
  maxPacketLifeTime?: number
  maxRetransmits?: number
  protocol?: string
  negotiated?: number
}

export interface RTCPeerConnectionIceEvent {
  candidate: RTCIceCandidate | null
}

export interface RTCErrorEvent {
  type: 'error'
  message: string
}

export interface RTCTrackEvent {
  track: MediaStreamTrack
  streams: MediaStream[]
}

export interface RTCDataChannelEvent {
  channel: RTCDataChannel
}

export interface MessageEvent<T = string | Buffer> {
  data: T
}

export interface RTCSessionDescriptionInit {
  type: RTCSdpType
  sdp: string
}
