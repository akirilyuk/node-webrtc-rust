/**
 * Signaling bridge between `@node-webrtc-rust/signaling` and `@node-webrtc-rust/conference`.
 *
 * ## Contract
 *
 * - Browser clients join rooms through {@link SignalingServer} using the standard
 *   `join` / `offer` / `answer` / `ice-candidate` wire messages.
 * - This module connects a server-side {@link SignalingClient} per conference room and
 *   translates those messages into native conference DTOs consumed by
 *   {@link ConferenceRoom.handleSignalingMessage}.
 * - Outbound native responses (`offer`, `answer`) are converted back to signaling wire
 *   messages and sent to the target participant.
 * - Consumer apps must not parse SDP or construct conference signaling JSON — attach this
 *   bridge via {@link ConferenceServer.attachSignaling} instead.
 */

import { SignalingClient } from '@node-webrtc-rust/signaling'
import type { IceCandidateEvent, OfferEvent, AnswerEvent } from '@node-webrtc-rust/signaling'

import type { ConferenceRoom } from './ConferenceRoom'
import type { ConferenceServer } from './ConferenceServer'
import { debugEvent, debugFn } from './debug'
import type { ConferenceSignalingResponse, SignalingBridgeConfig } from './types'

const MODULE = 'conference::signaling-bridge'
const DEFAULT_SERVER_PEER_ID = 'conference-server'

interface RoomBridgeState {
  client: SignalingClient
  handlers: {
    peerJoined: (peerId: string) => void
    peerLeft: (peerId: string) => void
    offer: (event: OfferEvent) => void
    answer: (event: AnswerEvent) => void
    iceCandidate: (event: IceCandidateEvent) => void
  }
}

const bridges = new WeakMap<ConferenceServer, Map<string, RoomBridgeState>>()

function getBridgeMap(server: ConferenceServer): Map<string, RoomBridgeState> {
  let map = bridges.get(server)
  if (!map) {
    map = new Map()
    bridges.set(server, map)
  }
  return map
}

function parseResponses(json: string): ConferenceSignalingResponse[] {
  if (!json || json === '[]') {
    return []
  }

  const parsed: unknown = JSON.parse(json)
  if (!Array.isArray(parsed)) {
    return [parsed as ConferenceSignalingResponse]
  }
  return parsed as ConferenceSignalingResponse[]
}

async function forwardResponses(
  room: ConferenceRoom,
  client: SignalingClient,
  targetPeerId: string,
  responsesJson: string,
): Promise<void> {
  const responses = parseResponses(responsesJson)
  for (const response of responses) {
    if (response.type === 'offer') {
      debugEvent(MODULE, 'forward-offer', `roomId=${room.roomId}, targetPeerId=${targetPeerId}`)
      client.sendOffer(targetPeerId, { type: 'offer', sdp: response.sdp })
    } else if (response.type === 'answer') {
      debugEvent(MODULE, 'forward-answer', `roomId=${room.roomId}, targetPeerId=${targetPeerId}`)
      client.sendAnswer(targetPeerId, { type: 'answer', sdp: response.sdp })
    }
  }
}

/**
 * Attaches signaling relay handlers for one conference room.
 *
 * @param server - Parent conference server owning the room map.
 * @param room - Room to wire to signaling.
 * @param config - Bridge configuration from {@link ConferenceServer.attachSignaling}.
 */
export async function attachSignalingBridge(
  server: ConferenceServer,
  room: ConferenceRoom,
  config: SignalingBridgeConfig,
): Promise<void> {
  const roomId = room.roomId
  const bridgeMap = getBridgeMap(server)

  if (bridgeMap.has(roomId)) {
    return
  }

  const serverPeerId = config.serverPeerId ?? DEFAULT_SERVER_PEER_ID
  debugFn(MODULE, 'attachSignalingBridge', `roomId=${roomId}, url=${config.url}`)

  const client = new SignalingClient({
    url: config.url,
    room: roomId,
    peerId: serverPeerId,
  })

  const onPeerJoined = (peerId: string): void => {
    if (peerId === serverPeerId) {
      return
    }

    void (async () => {
      debugEvent(MODULE, 'peer-joined', `roomId=${roomId}, peerId=${peerId}`)
      const responses = await room.handleSignalingMessage(
        JSON.stringify({
          type: 'join',
          participantId: peerId,
          roomId,
        }),
      )
      await forwardResponses(room, client, peerId, responses)
    })().catch((error: unknown) => {
      server.emit('error', {
        roomId,
        message: error instanceof Error ? error.message : String(error),
      })
    })
  }

  const onPeerLeft = (peerId: string): void => {
    if (peerId === serverPeerId) {
      return
    }

    void (async () => {
      debugEvent(MODULE, 'peer-left', `roomId=${roomId}, peerId=${peerId}`)
      await room.handleSignalingMessage(
        JSON.stringify({
          type: 'leave',
          participantId: peerId,
        }),
      )
    })().catch((error: unknown) => {
      server.emit('error', {
        roomId,
        message: error instanceof Error ? error.message : String(error),
      })
    })
  }

  const onOffer = ({ peerId, sdp }: OfferEvent): void => {
    if (peerId === serverPeerId) {
      return
    }

    void (async () => {
      debugEvent(MODULE, 'offer', `roomId=${roomId}, peerId=${peerId}`)
      const responses = await room.handleSignalingMessage(
        JSON.stringify({
          type: 'offer',
          participantId: peerId,
          sdp: sdp.sdp ?? '',
        }),
      )
      await forwardResponses(room, client, peerId, responses)
    })().catch((error: unknown) => {
      server.emit('error', {
        roomId,
        message: error instanceof Error ? error.message : String(error),
      })
    })
  }

  const onAnswer = ({ peerId, sdp }: AnswerEvent): void => {
    if (peerId === serverPeerId) {
      return
    }

    void (async () => {
      debugEvent(MODULE, 'answer', `roomId=${roomId}, peerId=${peerId}`)
      await room.handleSignalingMessage(
        JSON.stringify({
          type: 'answer',
          participantId: peerId,
          sdp: sdp.sdp ?? '',
        }),
      )
    })().catch((error: unknown) => {
      server.emit('error', {
        roomId,
        message: error instanceof Error ? error.message : String(error),
      })
    })
  }

  const onIceCandidate = ({ peerId, candidate }: IceCandidateEvent): void => {
    if (peerId === serverPeerId || !candidate.candidate) {
      return
    }

    void (async () => {
      debugEvent(MODULE, 'ice-candidate', `roomId=${roomId}, peerId=${peerId}`)
      await room.handleSignalingMessage(
        JSON.stringify({
          type: 'iceCandidate',
          participantId: peerId,
          candidate: candidate.candidate,
          sdpMid: candidate.sdpMid,
          sdpMLineIndex: candidate.sdpMLineIndex,
        }),
      )
    })().catch((error: unknown) => {
      server.emit('error', {
        roomId,
        message: error instanceof Error ? error.message : String(error),
      })
    })
  }

  client.on('peer-joined', onPeerJoined)
  client.on('peer-left', onPeerLeft)
  client.on('offer', onOffer)
  client.on('answer', onAnswer)
  client.on('ice-candidate', onIceCandidate)

  await client.connect()

  bridgeMap.set(roomId, {
    client,
    handlers: {
      peerJoined: onPeerJoined,
      peerLeft: onPeerLeft,
      offer: onOffer,
      answer: onAnswer,
      iceCandidate: onIceCandidate,
    },
  })
}

/**
 * Detaches and disconnects the signaling bridge for one room.
 *
 * @param server - Parent conference server.
 * @param roomId - Room whose bridge client should be torn down.
 */
export async function detachSignalingBridge(server: ConferenceServer, roomId: string): Promise<void> {
  const bridgeMap = bridges.get(server)
  const state = bridgeMap?.get(roomId)
  if (!state) {
    return
  }

  debugFn(MODULE, 'detachSignalingBridge', `roomId=${roomId}`)
  state.client.off('peer-joined', state.handlers.peerJoined)
  state.client.off('peer-left', state.handlers.peerLeft)
  state.client.off('offer', state.handlers.offer)
  state.client.off('answer', state.handlers.answer)
  state.client.off('ice-candidate', state.handlers.iceCandidate)
  state.client.disconnect()
  bridgeMap?.delete(roomId)
}
