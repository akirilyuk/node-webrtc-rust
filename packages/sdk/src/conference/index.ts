/**
 * @packageDocumentation
 * Conference control-plane API for node-webrtc-rust.
 *
 * Import from `@node-webrtc-rust/sdk/conference` to manage rooms, participant mutes,
 * and signaling bridges without handling audio buffers in TypeScript.
 */
export { ConferenceServer } from './ConferenceServer'
export { ConferenceRoom } from './ConferenceRoom'
export { attachSignalingBridge, detachSignalingBridge } from './signaling-bridge'
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

export { version } from '@node-webrtc-rust/bindings'
