/**
 * @packageDocumentation
 * Browser-compatible WebRTC API for Node.js.
 *
 * Import from `@node-webrtc-rust/sdk` to create peer connections, data channels,
 * and local audio tracks without touching the native bindings directly.
 */
export { RTCPeerConnection } from './RTCPeerConnection'
export { RTCSessionDescription } from './RTCSessionDescription'
export { RTCIceCandidate } from './RTCIceCandidate'
export { RTCDataChannel } from './RTCDataChannel'
export { RTCRtpReceiver } from './RTCRtpReceiver'
export { RTCRtpSender } from './RTCRtpSender'
export { RTCRtpTransceiver } from './RTCRtpTransceiver'
export { MediaStream } from './MediaStream'
export { MediaStreamTrack } from './MediaStreamTrack'
export { LocalAudioTrack } from './LocalAudioTrack'
export { RemoteAudioTrack } from './RemoteAudioTrack'
export { debugEvent, debugFn, isDebugEnabled, setDebugEnabled } from './debug'
export type {
  RTCConfiguration,
  RTCIceServer,
  RTCDataChannelInit,
  RTCPeerConnectionState,
  RTCIceConnectionState,
  RTCIceGatheringState,
  RTCSignalingState,
  RTCSdpType,
  RTCIceCandidateInit,
  RTCSessionDescriptionInit,
  RTCAnswerOptions,
  RTCOfferOptions,
  RTCRtpTransceiverDirection,
  RTCRtpTransceiverInit,
  RTCStatsReport,
  RTCPeerConnectionIceEvent,
  RTCTrackEvent,
  RTCDataChannelEvent,
  MessageEvent,
  TrackKind,
} from './types'

/** Native bindings crate version string. */
export { version } from '@node-webrtc-rust/bindings'
