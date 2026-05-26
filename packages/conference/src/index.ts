/**
 * @packageDocumentation
 * Conference control-plane API for node-webrtc-rust.
 *
 * Import from `@node-webrtc-rust/conference` to manage rooms, participant mutes,
 * and signaling bridges without handling audio buffers in TypeScript.
 */
export { ConferenceServer } from './ConferenceServer'
export { ConferenceRoom } from './ConferenceRoom'
export { attachSignalingBridge, detachSignalingBridge } from './signaling-bridge'
export { debugEvent, debugFn, isDebugEnabled, setDebugEnabled } from './debug'
export type {
  MuteScope,
  MuteOptions,
  RoomOptions,
  RTCIceServer,
  ParticipantInfo,
  RoomCreatedEvent,
  RoomDestroyedEvent,
  ParticipantJoinedEvent,
  ParticipantLeftEvent,
  ParticipantKickedEvent,
  ParticipantMutedEvent,
  MixingEnabledChangedEvent,
  ConferenceErrorEvent,
  SignalingBridgeConfig,
  ConferenceSignalingResponse,
} from './types'

/** Native conference bindings version string. */
export { version } from '@node-webrtc-rust/conference-bindings'
