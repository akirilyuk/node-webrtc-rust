import {
  RTCIceCandidate,
  RTCSessionDescription,
  type RTCIceCandidateInit,
  type RTCSessionDescriptionInit,
} from '@node-webrtc-rust/sdk'

import type { AutoNegotiateOptions } from './types'

export function autoNegotiate(options: AutoNegotiateOptions): () => void {
  const { pc, signaling, polite } = options
  const knownPeers = new Set<string>()
  let makingOffer = false

  const onPeerJoined = async (peerId: string) => {
    if (peerId === signaling.peerId || knownPeers.has(peerId)) {
      return
    }
    knownPeers.add(peerId)
    wireIceToPeer(peerId)
    if (!polite) {
      makingOffer = true
      try {
        const offer = await pc.createOffer()
        await pc.setLocalDescription(offer)
        signaling.sendOffer(peerId, offer.toJSON())
      } finally {
        makingOffer = false
      }
    }
  }

  const wireIceToPeer = (peerId: string) => {
    pc.onicecandidate = (event) => {
      if (event.candidate) {
        signaling.sendIceCandidate(peerId, event.candidate.toJSON())
      }
    }
  }

  const onOffer = async ({ peerId, sdp }: { peerId: string; sdp: RTCSessionDescriptionInit }) => {
    if (!polite && makingOffer) {
      return
    }
    wireIceToPeer(peerId)
    await pc.setRemoteDescription(new RTCSessionDescription(sdp))
    const answer = await pc.createAnswer()
    await pc.setLocalDescription(answer)
    signaling.sendAnswer(peerId, answer.toJSON())
  }

  const onAnswer = async ({ sdp }: { peerId: string; sdp: RTCSessionDescriptionInit }) => {
    await pc.setRemoteDescription(new RTCSessionDescription(sdp))
  }

  const onRemoteIce = async ({ candidate }: { peerId: string; candidate: RTCIceCandidateInit }) => {
    if (candidate.candidate) {
      await pc.addIceCandidate(new RTCIceCandidate(candidate))
    }
  }

  signaling.on('peer-joined', onPeerJoined)
  signaling.on('offer', onOffer)
  signaling.on('answer', onAnswer)
  signaling.on('ice-candidate', onRemoteIce)

  return () => {
    signaling.off('peer-joined', onPeerJoined)
    signaling.off('offer', onOffer)
    signaling.off('answer', onAnswer)
    signaling.off('ice-candidate', onRemoteIce)
  }
}

export type { AutoNegotiateOptions } from './types'
