export { SignalingServer } from './server'
export { SignalingClient } from './client'
export { autoNegotiate } from './auto-negotiate'
export { assertSdpHasIceCredentials, describeSdpIce } from './sdp-ice-guard'
export type { SdpIceKind, SdpIceMeta } from './sdp-ice-guard'
export {
  ConnectionError,
  createConnectionError,
  dispatchConnectionError,
  formatConnectionErrorSource,
  getRootConnectionErrorHandler,
  reportConnectionError,
  setRootConnectionErrorHandler,
} from '@node-webrtc-rust/sdk'
export type {
  ConnectionErrorSource,
  RootConnectionErrorHandler,
  SessionErrorSource,
  SignalingErrorSource,
  WebRtcErrorSource,
} from '@node-webrtc-rust/sdk'
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
