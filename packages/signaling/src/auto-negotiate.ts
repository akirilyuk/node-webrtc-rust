import {
  RTCIceCandidate,
  RTCSessionDescription,
  type RTCIceCandidateInit,
  type RTCPeerConnectionIceEvent,
  type RTCSessionDescriptionInit,
} from '@node-webrtc-rust/sdk'

import { debugFn } from './debug'
import type { AutoNegotiateOptions } from './types'

/**
 * Wires a {@link RTCPeerConnection} to a {@link SignalingClient} for automatic
 * SDP and trickle ICE exchange.
 *
 * The impolite peer (`polite: false`) creates an offer when a remote peer joins.
 * The polite peer (`polite: true`) answers incoming offers and handles offer glare
 * by ignoring concurrent offers while making its own.
 *
 * @param options - Peer connection, connected signaling client, and role.
 * @returns Teardown function that removes all signaling listeners.
 */
export function autoNegotiate(options: AutoNegotiateOptions): () => void {
  const { pc, signaling, polite } = options
  debugFn('signaling::autoNegotiate', 'start', `polite=${polite}`)
  const knownPeers = new Set<string>()
  let makingOffer = false

  const onPeerJoined = async (peerId: string) => {
    debugFn('signaling::autoNegotiate', 'onPeerJoined', `peerId=${peerId}`)
    if (peerId === signaling.peerId || knownPeers.has(peerId)) {
      return
    }
    knownPeers.add(peerId)
    wireIceToPeer(peerId)
    if (!polite) {
      makingOffer = true
      try {
        debugFn('signaling::autoNegotiate', 'createOffer', `peerId=${peerId}`)
        const offer = await pc.createOffer()
        await pc.setLocalDescription(offer)
        await pc.gatheringComplete()
        signaling.sendOffer(peerId, pc.localDescription!.toJSON())
      } finally {
        makingOffer = false
      }
    }
  }

  const wireIceToPeer = (peerId: string) => {
    pc.onicecandidate = (event: RTCPeerConnectionIceEvent) => {
      if (event.candidate) {
        signaling.sendIceCandidate(peerId, event.candidate.toJSON())
      }
    }
  }

  const onOffer = async ({ peerId, sdp }: { peerId: string; sdp: RTCSessionDescriptionInit }) => {
    debugFn('signaling::autoNegotiate', 'onOffer', `peerId=${peerId}`)
    if (!polite && makingOffer) {
      return
    }
    wireIceToPeer(peerId)
    await pc.setRemoteDescription(new RTCSessionDescription(sdp))
    const answer = await pc.createAnswer()
    await pc.setLocalDescription(answer)
    await pc.gatheringComplete()
    signaling.sendAnswer(peerId, pc.localDescription!.toJSON())
  }

  const onAnswer = async ({ peerId, sdp }: { peerId: string; sdp: RTCSessionDescriptionInit }) => {
    debugFn('signaling::autoNegotiate', 'onAnswer', `peerId=${peerId}`)
    await pc.setRemoteDescription(new RTCSessionDescription(sdp))
  }

  const onRemoteIce = async ({
    peerId,
    candidate,
  }: {
    peerId: string
    candidate: RTCIceCandidateInit
  }) => {
    debugFn('signaling::autoNegotiate', 'onRemoteIce', `peerId=${peerId}`)
    if (candidate.candidate) {
      await pc.addIceCandidate(new RTCIceCandidate(candidate))
    }
  }

  signaling.on('peer-joined', onPeerJoined)
  signaling.on('offer', onOffer)
  signaling.on('answer', onAnswer)
  signaling.on('ice-candidate', onRemoteIce)

  return () => {
    debugFn('signaling::autoNegotiate', 'teardown')
    signaling.off('peer-joined', onPeerJoined)
    signaling.off('offer', onOffer)
    signaling.off('answer', onAnswer)
    signaling.off('ice-candidate', onRemoteIce)
  }
}

export type { AutoNegotiateOptions } from './types'
