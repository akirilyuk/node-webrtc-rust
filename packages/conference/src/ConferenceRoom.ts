import {
  JsConferenceRoom,
  JsMuteScope,
  type JsIceServer,
  type JsMuteOptions,
  type JsParticipantInfo,
  type JsRoomOptions,
} from '@node-webrtc-rust/conference-bindings'

import type { ConferenceServer } from './ConferenceServer'
import { debugEvent, debugFn } from './debug'
import type {
  MuteOptions,
  ParticipantInfo,
  RTCIceServer,
  RoomOptions,
} from './types'

const MODULE = 'conference::ConferenceRoom'

/**
 * Validates mute options before forwarding them to the native room.
 * @throws Error when `scope` is `'listener'` and `listenerId` is missing.
 */
export function validateMuteOptions(options: MuteOptions): void {
  if (options.scope === 'listener' && !options.listenerId) {
    throw new Error('listenerId is required when mute scope is "listener"')
  }
}

function toJsMuteOptions(options: MuteOptions): JsMuteOptions {
  return {
    scope: options.scope === 'global' ? JsMuteScope.Global : JsMuteScope.Listener,
    listenerId: options.listenerId,
  }
}

function toJsRoomOptions(options?: RoomOptions): JsRoomOptions | undefined {
  if (!options) {
    return undefined
  }

  return {
    maxParticipants: options.maxParticipants,
    iceServers: options.iceServers?.map(toJsIceServer),
  }
}

function toJsIceServer(server: RTCIceServer): JsIceServer {
  return {
    urls: Array.isArray(server.urls) ? server.urls : [server.urls],
    username: server.username,
    credential: server.credential,
    credentialType: server.credentialType,
  }
}

function toParticipantInfo(info: JsParticipantInfo): ParticipantInfo {
  return {
    id: info.id,
    connectionState: info.connectionState,
  }
}

/**
 * Control-plane handle for one conference room.
 *
 * Wraps the native {@link JsConferenceRoom} with typed mute/kick APIs and forwards
 * lifecycle events to the parent {@link ConferenceServer}.
 */
export class ConferenceRoom {
  /** Room identifier this handle manages. */
  readonly roomId: string

  private readonly native: JsConferenceRoom
  private readonly server: ConferenceServer
  private eventsWired = false

  /** @internal */
  constructor(roomId: string, native: JsConferenceRoom, server: ConferenceServer) {
    this.roomId = roomId
    this.native = native
    this.server = server
  }

  /**
   * Returns participant summaries for admin or UI list APIs.
   * @returns Connected participants with connection state strings.
   */
  async listParticipants(): Promise<ParticipantInfo[]> {
    debugFn(MODULE, 'listParticipants', `roomId=${this.roomId}`)
    const participants = await this.native.listParticipants()
    return participants.map(toParticipantInfo)
  }

  /**
   * Mutes a participant globally or for one listener.
   *
   * **Auth:** In production, restrict global mutes to admins or moderators.
   * Listener-scoped mutes are typically allowed for the owning client as a preference.
   *
   * @param targetId - Participant to mute.
   * @param options - Mute scope; `listenerId` is required when `scope` is `'listener'`.
   */
  async muteParticipant(targetId: string, options: MuteOptions): Promise<void> {
    debugFn(
      MODULE,
      'muteParticipant',
      `roomId=${this.roomId}, targetId=${targetId}, scope=${options.scope}`,
    )
    validateMuteOptions(options)
    await this.native.muteParticipant(targetId, toJsMuteOptions(options))
  }

  /**
   * Unmutes a participant globally or for one listener.
   *
   * @param targetId - Participant to unmute.
   * @param options - Mute scope; `listenerId` is required when `scope` is `'listener'`.
   */
  async unmuteParticipant(targetId: string, options: MuteOptions): Promise<void> {
    debugFn(
      MODULE,
      'unmuteParticipant',
      `roomId=${this.roomId}, targetId=${targetId}, scope=${options.scope}`,
    )
    validateMuteOptions(options)
    await this.native.unmuteParticipant(targetId, toJsMuteOptions(options))
  }

  /**
   * Enables or disables room-wide mixing.
   *
   * When mixing is disabled, all listeners receive silence while participant mute
   * state is preserved.
   *
   * **Auth:** In production, restrict this to admins or moderators.
   *
   * @param enabled - Whether the mixer should produce audio for the room.
   */
  async setMixingEnabled(enabled: boolean): Promise<void> {
    debugFn(MODULE, 'setMixingEnabled', `roomId=${this.roomId}, enabled=${enabled}`)
    await this.native.setMixingEnabled(enabled)
  }

  /**
   * Returns whether room-wide mixing is currently enabled.
   * @returns `true` when the mixer produces personalized output for listeners.
   */
  async isMixingEnabled(): Promise<boolean> {
    debugFn(MODULE, 'isMixingEnabled', `roomId=${this.roomId}`)
    return this.native.isMixingEnabled()
  }

  /**
   * Removes a participant from the room with an optional kick reason.
   *
   * **Auth:** In production, restrict kicks to admins or moderators.
   *
   * @param participantId - Participant to remove.
   * @param reason - Optional human-readable kick reason for logs or UI.
   */
  async kickParticipant(participantId: string, reason?: string): Promise<void> {
    debugFn(MODULE, 'kickParticipant', `roomId=${this.roomId}, participantId=${participantId}`)
    await this.native.kickParticipant(participantId, reason)
  }

  /**
   * Routes a JSON signaling payload through the native room.
   *
   * Used by {@link attachSignalingBridge}; consumer apps should not parse SDP manually.
   *
   * @param json - Conference signaling DTO JSON (`join`, `offer`, `answer`, `iceCandidate`, `leave`).
   * @returns JSON array string of outbound signaling responses (`offer` / `answer`).
   */
  async handleSignalingMessage(json: string): Promise<string> {
    debugFn(MODULE, 'handleSignalingMessage', `roomId=${this.roomId}`)
    return this.native.handleSignalingMessage(json)
  }

  /**
   * Closes all participants and clears room state.
   *
   * Prefer {@link ConferenceServer.destroyRoom} when removing a room from the server map.
   */
  async close(): Promise<void> {
    debugFn(MODULE, 'close', `roomId=${this.roomId}`)
    await this.native.close()
    await this.server.notifyRoomClosed(this.roomId)
  }

  /** @internal Returns the native room handle for bridge wiring. */
  getNativeRoom(): JsConferenceRoom {
    return this.native
  }

  /** @internal Registers native callbacks and forwards them to {@link ConferenceServer}. */
  wireEvents(): void {
    if (this.eventsWired) {
      return
    }
    this.eventsWired = true

    this.native.setOnParticipantJoined((event) => {
      debugEvent(MODULE, 'participant-joined', `roomId=${event.roomId}, participantId=${event.participantId}`)
      this.server.emitParticipantJoined(event)
    })

    this.native.setOnParticipantLeft((event) => {
      debugEvent(MODULE, 'participant-left', `roomId=${event.roomId}, participantId=${event.participantId}`)
      this.server.emitParticipantLeft(event)
    })

    this.native.setOnParticipantKicked((event) => {
      debugEvent(MODULE, 'participant-kicked', `roomId=${event.roomId}, participantId=${event.participantId}`)
      this.server.emitParticipantKicked(event)
    })

    this.native.setOnParticipantMuted((event) => {
      debugEvent(MODULE, 'participant-muted', `roomId=${event.roomId}, targetId=${event.targetId}`)
      this.server.emitParticipantMuted(event)
    })

    this.native.setOnMixingEnabledChanged((event) => {
      debugEvent(MODULE, 'mixing-enabled-changed', `roomId=${event.roomId}, enabled=${event.enabled}`)
      this.server.emitMixingEnabledChanged(event)
    })

    this.native.setOnError((event) => {
      debugEvent(MODULE, 'error', `roomId=${event.roomId ?? this.roomId}, message=${event.message}`)
      this.server.emitError(event)
    })
  }
}

export { toJsRoomOptions }
