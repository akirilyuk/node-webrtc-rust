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
export { assertSdpHasIceCredentials, describeSdpIce } from './sdp-ice-guard'
export type { SdpIceKind, SdpIceMeta } from './sdp-ice-guard'
export {
  ensureLocalDescriptionHasIceCredentials,
  LOCAL_DESCRIPTION_ICE_MAX_RETRIES,
} from './local-description-ice'
export type {
  LocalDescriptionIceRetryDeps,
  LocalDescriptionIceRetryOptions,
} from './local-description-ice'
export {
  ConnectionError,
  createConnectionError,
  dispatchConnectionError,
  formatConnectionErrorSource,
  getRootConnectionErrorHandler,
  reportConnectionError,
  setRootConnectionErrorHandler,
} from './connection-errors'
export type {
  ConnectionErrorSource,
  RootConnectionErrorHandler,
  SessionErrorSource,
  SignalingErrorSource,
  WebRtcErrorSource,
} from './connection-errors'
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
