import { EventEmitter } from 'events'

import {
  JsConferenceServer,
  type JsConferenceRoom,
  type JsMixingEnabledChangedEvent,
  type JsParticipantEvent,
  type JsParticipantKickedEvent,
  type JsParticipantMutedEvent,
  type JsRoomErrorEvent,
} from '@node-webrtc-rust/bindings'

import { ConferenceRoom, toJsRoomOptions } from './ConferenceRoom'
import { debugEvent, debugFn } from '../debug'
import { attachSignalingBridge, detachSignalingBridge } from './signaling-bridge'
import type {
  ConferenceErrorEvent,
  MixingEnabledChangedEvent,
  ParticipantJoinedEvent,
  ParticipantKickedEvent,
  ParticipantLeftEvent,
  ParticipantMutedEvent,
  RoomCreatedEvent,
  RoomDestroyedEvent,
  RoomOptions,
  SignalingBridgeConfig,
} from './types'

const MODULE = 'conference::ConferenceServer'

/**
 * Conference control-plane server managing multiple rooms.
 *
 * Wraps the native {@link JsConferenceServer}, exposes typed room lifecycle APIs,
 * and emits room/participant events for application logic and admin UIs.
 */
export class ConferenceServer extends EventEmitter {
  private readonly native = new JsConferenceServer()
  private readonly rooms = new Map<string, ConferenceRoom>()
  private signalingBridge?: SignalingBridgeConfig

  /**
   * Creates a conference room and returns a control handle.
   *
   * @param roomId - Unique room identifier (typically matches the signaling room name).
   * @param options - Optional participant limit and ICE servers for server-side PCs.
   * @returns A {@link ConferenceRoom} handle wired to this server.
   */
  async createRoom(roomId: string, options?: RoomOptions): Promise<ConferenceRoom> {
    debugFn(MODULE, 'createRoom', `roomId=${roomId}`)
    const native = await this.native.createRoom(roomId, toJsRoomOptions(options))
    const room = this.registerRoom(roomId, native)
    debugEvent(MODULE, 'room-created', `roomId=${roomId}`)
    this.emit('room-created', { roomId } satisfies RoomCreatedEvent)
    return room
  }

  /**
   * Returns an existing room handle, if the room is active.
   *
   * @param roomId - Room identifier to look up.
   * @returns The room handle, or `undefined` when the room does not exist.
   */
  async getRoom(roomId: string): Promise<ConferenceRoom | undefined> {
    debugFn(MODULE, 'getRoom', `roomId=${roomId}`)
    const cached = this.rooms.get(roomId)
    if (cached) {
      return cached
    }

    const native = await this.native.getRoom(roomId)
    if (!native) {
      return undefined
    }

    return this.registerRoom(roomId, native)
  }

  /**
   * Lists active room identifiers managed by this server.
   * @returns Room ids currently held by the native server.
   */
  async listRooms(): Promise<string[]> {
    debugFn(MODULE, 'listRooms')
    return this.native.listRooms()
  }

  /**
   * Destroys a room and tears down all participants and mixer state.
   *
   * @param roomId - Room to destroy.
   */
  async destroyRoom(roomId: string): Promise<void> {
    debugFn(MODULE, 'destroyRoom', `roomId=${roomId}`)
    await detachSignalingBridge(this, roomId)
    this.rooms.delete(roomId)
    await this.native.destroyRoom(roomId)
    debugEvent(MODULE, 'room-destroyed', `roomId=${roomId}`)
    this.emit('room-destroyed', { roomId } satisfies RoomDestroyedEvent)
  }

  /**
   * Wires a signaling WebSocket relay to this server.
   *
   * When attached, each created or retrieved room gets a server-side signaling client
   * that translates WS messages into native conference signaling DTOs.
   *
   * @param signaling - Bridge configuration with the signaling server WebSocket URL.
   */
  attachSignaling(signaling: SignalingBridgeConfig): void {
    debugFn(MODULE, 'attachSignaling', `url=${signaling.url}`)
    this.signalingBridge = signaling
    void this.wireExistingRooms()
  }

  /** @internal Called when a room is closed via {@link ConferenceRoom.close}. */
  async notifyRoomClosed(roomId: string): Promise<void> {
    await detachSignalingBridge(this, roomId)
    this.rooms.delete(roomId)
    debugEvent(MODULE, 'room-destroyed', `roomId=${roomId}`)
    this.emit('room-destroyed', { roomId } satisfies RoomDestroyedEvent)
  }

  /** @internal Returns bridge config when {@link attachSignaling} was called. */
  getSignalingBridgeConfig(): SignalingBridgeConfig | undefined {
    return this.signalingBridge
  }

  /** @internal Returns a cached room handle when present. */
  getCachedRoom(roomId: string): ConferenceRoom | undefined {
    return this.rooms.get(roomId)
  }

  /** @internal Forwards native participant joined events to listeners. */
  emitParticipantJoined(event: JsParticipantEvent): void {
    this.emit('participant-joined', {
      roomId: event.roomId,
      participantId: event.participantId,
    } satisfies ParticipantJoinedEvent)
  }

  /** @internal Forwards native participant left events to listeners. */
  emitParticipantLeft(event: JsParticipantEvent): void {
    this.emit('participant-left', {
      roomId: event.roomId,
      participantId: event.participantId,
    } satisfies ParticipantLeftEvent)
  }

  /** @internal Forwards native participant kicked events to listeners. */
  emitParticipantKicked(event: JsParticipantKickedEvent): void {
    this.emit('participant-kicked', {
      roomId: event.roomId,
      participantId: event.participantId,
      reason: event.reason,
    } satisfies ParticipantKickedEvent)
  }

  /** @internal Forwards native participant muted events to listeners. */
  emitParticipantMuted(event: JsParticipantMutedEvent): void {
    this.emit('participant-muted', {
      roomId: event.roomId,
      targetId: event.targetId,
      scope: event.scope,
      listenerId: event.listenerId,
    } satisfies ParticipantMutedEvent)
  }

  /** @internal Forwards native mixing enabled changes to listeners. */
  emitMixingEnabledChanged(event: JsMixingEnabledChangedEvent): void {
    this.emit('mixing-enabled-changed', {
      roomId: event.roomId,
      enabled: event.enabled,
    } satisfies MixingEnabledChangedEvent)
  }

  /** @internal Forwards native room errors to listeners. */
  emitError(event: JsRoomErrorEvent): void {
    this.emit('error', {
      roomId: event.roomId,
      message: event.message,
      code: event.code,
    } satisfies ConferenceErrorEvent)
  }

  private registerRoom(roomId: string, native: JsConferenceRoom): ConferenceRoom {
    const room = new ConferenceRoom(roomId, native, this)
    room.wireEvents()
    this.rooms.set(roomId, room)

    if (this.signalingBridge) {
      void attachSignalingBridge(this, room, this.signalingBridge)
    }

    return room
  }

  private async wireExistingRooms(): Promise<void> {
    if (!this.signalingBridge) {
      return
    }

    for (const room of this.rooms.values()) {
      await attachSignalingBridge(this, room, this.signalingBridge)
    }
  }
}

export declare interface ConferenceServer {
  on(event: 'room-created', listener: (payload: RoomCreatedEvent) => void): this
  on(event: 'room-destroyed', listener: (payload: RoomDestroyedEvent) => void): this
  on(event: 'participant-joined', listener: (payload: ParticipantJoinedEvent) => void): this
  on(event: 'participant-left', listener: (payload: ParticipantLeftEvent) => void): this
  on(event: 'participant-kicked', listener: (payload: ParticipantKickedEvent) => void): this
  on(event: 'participant-muted', listener: (payload: ParticipantMutedEvent) => void): this
  on(event: 'mixing-enabled-changed', listener: (payload: MixingEnabledChangedEvent) => void): this
  on(event: 'error', listener: (payload: ConferenceErrorEvent) => void): this
  emit(event: 'room-created', payload: RoomCreatedEvent): boolean
  emit(event: 'room-destroyed', payload: RoomDestroyedEvent): boolean
  emit(event: 'participant-joined', payload: ParticipantJoinedEvent): boolean
  emit(event: 'participant-left', payload: ParticipantLeftEvent): boolean
  emit(event: 'participant-kicked', payload: ParticipantKickedEvent): boolean
  emit(event: 'participant-muted', payload: ParticipantMutedEvent): boolean
  emit(event: 'mixing-enabled-changed', payload: MixingEnabledChangedEvent): boolean
  emit(event: 'error', payload: ConferenceErrorEvent): boolean
}
