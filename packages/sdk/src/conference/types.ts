/** Scope for participant mute operations. */
export type MuteScope = 'global' | 'listener'

/** Options for {@link ConferenceRoom.muteParticipant} and {@link ConferenceRoom.unmuteParticipant}. */
export interface MuteOptions {
  /** Whether the mute applies to all listeners or one listener only. */
  scope: MuteScope
  /** Required when {@link MuteOptions.scope} is `'listener'`. */
  listenerId?: string
}

/** ICE server entry for conference room peer connections. */
export interface RTCIceServer {
  urls: string | string[]
  username?: string
  credential?: string
  credentialType?: 'password'
}

/** Options passed to {@link ConferenceServer.createRoom}. */
export interface RoomOptions {
  /** Maximum number of participants allowed in the room. */
  maxParticipants?: number
  /** ICE servers used when creating server-side peer connections. */
  iceServers?: RTCIceServer[]
}

/** Summary of one connected participant. */
export interface ParticipantInfo {
  /** Stable participant identifier (signaling peer id). */
  id: string
  /** WebRTC connection state string from the native engine. */
  connectionState: string
}

/** Payload for the `room-created` event. */
export interface RoomCreatedEvent {
  roomId: string
}

/** Payload for the `room-destroyed` event. */
export interface RoomDestroyedEvent {
  roomId: string
}

/** Payload for participant lifecycle events. */
export interface ParticipantJoinedEvent {
  roomId: string
  participantId: string
}

/** Payload for the `participant-left` event. */
export interface ParticipantLeftEvent {
  roomId: string
  participantId: string
}

/** Payload for the `participant-kicked` event. */
export interface ParticipantKickedEvent {
  roomId: string
  participantId: string
  reason?: string
}

/** Payload for the `participant-muted` event. */
export interface ParticipantMutedEvent {
  roomId: string
  targetId: string
  scope: MuteScope
  listenerId?: string
}

/** Payload for the `mixing-enabled-changed` event. */
export interface MixingEnabledChangedEvent {
  roomId: string
  enabled: boolean
}

/** Payload for the `error` event. */
export interface ConferenceErrorEvent {
  roomId?: string
  message: string
  code?: string
}

/** Configuration for wiring a {@link SignalingServer} to {@link ConferenceServer}. */
export interface SignalingBridgeConfig {
  /** WebSocket URL of the attached signaling server, e.g. `ws://localhost:3000/ws`. */
  url: string
  /**
   * Peer id used by the server-side bridge client in each room.
   * Defaults to `conference-server`.
   */
  serverPeerId?: string
}

/** Outbound signaling response produced by the native room (bridge-internal). */
export interface ConferenceSignalingResponse {
  type: 'offer' | 'answer'
  participantId: string
  sdp: string
}
