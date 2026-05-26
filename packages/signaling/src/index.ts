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

export const SIGNALING_VERSION = '0.1.0'
