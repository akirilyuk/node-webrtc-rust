import { EventEmitter } from 'events'

import {
  JsPeerConnection as NativePeerConnection,
  type JsRtcIceCandidate,
  type JsRtcSessionDescription,
} from '@node-webrtc-rust/bindings'

import { MediaStream } from './MediaStream'
import { MediaStreamTrack } from './MediaStreamTrack'
import { RTCDataChannel } from './RTCDataChannel'
import { RTCIceCandidate } from './RTCIceCandidate'
import { RTCSessionDescription } from './RTCSessionDescription'
import type {
  RTCConfiguration,
  RTCDataChannelEvent,
  RTCDataChannelInit,
  RTCIceConnectionState,
  RTCIceCandidateInit,
  RTCPeerConnectionIceEvent,
  RTCPeerConnectionState,
  RTCOfferOptions,
  RTCTrackEvent,
} from './types'

function toNativeConfig(config?: RTCConfiguration) {
  if (!config) return undefined
  return {
    iceServers: config.iceServers?.map((server) => ({
      urls: Array.isArray(server.urls) ? server.urls : [server.urls],
      username: server.username,
      credential: server.credential,
    })),
    iceTransportPolicy: config.iceTransportPolicy,
  }
}

function toNativeDescription(desc: RTCSessionDescription): JsRtcSessionDescription {
  return { type: desc.type, sdp: desc.sdp }
}

function fromNativeDescription(desc: JsRtcSessionDescription): RTCSessionDescription {
  return new RTCSessionDescription({
    type: desc.type as RTCSessionDescription['type'],
    sdp: desc.sdp,
  })
}

export class RTCPeerConnection extends EventEmitter {
  private readonly native: NativePeerConnection
  private _localDescription: RTCSessionDescription | null = null
  private _remoteDescription: RTCSessionDescription | null = null

  onicecandidate: ((event: RTCPeerConnectionIceEvent) => void) | null = null
  ontrack: ((event: RTCTrackEvent) => void) | null = null
  ondatachannel: ((event: RTCDataChannelEvent) => void) | null = null
  onconnectionstatechange: ((event: Event) => void) | null = null
  oniceconnectionstatechange: ((event: Event) => void) | null = null
  onnegotiationneeded: ((event: Event) => void) | null = null

  constructor(config?: RTCConfiguration) {
    super()
    this.native = new NativePeerConnection(toNativeConfig(config))

    this.native.setOnIceCandidate((candidate) => {
      const event: RTCPeerConnectionIceEvent = {
        candidate: candidate
          ? new RTCIceCandidate({
              candidate: candidate.candidate,
              sdpMid: candidate.sdpMid ?? null,
              sdpMLineIndex: candidate.sdpMLineIndex ?? null,
              usernameFragment: candidate.usernameFragment ?? null,
            })
          : null,
      }
      this.onicecandidate?.(event)
      this.emit('icecandidate', event)
    })

    this.native.setOnTrack((track) => {
      const wrappedTrack = new MediaStreamTrack(track)
      const stream = MediaStream.fromNativeTrack(track)
      const event: RTCTrackEvent = { track: wrappedTrack, streams: [stream] }
      this.ontrack?.(event)
      this.emit('track', event)
    })

    this.native.setOnDataChannel((channel) => {
      const wrapped = new RTCDataChannel(channel)
      const event: RTCDataChannelEvent = { channel: wrapped }
      this.ondatachannel?.(event)
      this.emit('datachannel', event)
    })

    this.native.setOnConnectionStateChange((state) => {
      const event = new Event('connectionstatechange')
      void state
      this.onconnectionstatechange?.(event)
      this.emit('connectionstatechange', event)
    })

    this.native.setOnIceConnectionStateChange((_state) => {
      const event = new Event('iceconnectionstatechange')
      this.oniceconnectionstatechange?.(event)
      this.emit('iceconnectionstatechange', event)
    })
  }

  async createOffer(_options?: RTCOfferOptions): Promise<RTCSessionDescription> {
    void _options
    return fromNativeDescription(await this.native.createOffer())
  }

  async createAnswer(): Promise<RTCSessionDescription> {
    return fromNativeDescription(await this.native.createAnswer())
  }

  async setLocalDescription(desc: RTCSessionDescription): Promise<void> {
    await this.native.setLocalDescription(toNativeDescription(desc))
    this._localDescription = desc
  }

  async setRemoteDescription(desc: RTCSessionDescription): Promise<void> {
    await this.native.setRemoteDescription(toNativeDescription(desc))
    this._remoteDescription = desc
  }

  async addIceCandidate(candidate: RTCIceCandidate | RTCIceCandidateInit): Promise<void> {
    const init = candidate instanceof RTCIceCandidate ? candidate.toJSON() : candidate
    const nativeCandidate: JsRtcIceCandidate = {
      candidate: init.candidate ?? '',
      sdpMid: init.sdpMid ?? undefined,
      sdpMLineIndex: init.sdpMLineIndex ?? undefined,
      usernameFragment: init.usernameFragment ?? undefined,
    }
    await this.native.addIceCandidate(nativeCandidate)
  }

  createDataChannel(label: string, options?: RTCDataChannelInit): RTCDataChannel {
    return RTCDataChannel.fromNativePromise(
      this.native.createDataChannel(label, options),
      label,
      options,
    )
  }

  close(): void {
    void this.native.close()
  }

  get localDescription(): RTCSessionDescription | null {
    return this._localDescription
  }

  get remoteDescription(): RTCSessionDescription | null {
    return this._remoteDescription
  }

  get connectionState(): RTCPeerConnectionState {
    return this.native.connectionState as RTCPeerConnectionState
  }

  get iceConnectionState(): RTCIceConnectionState {
    return this.native.iceConnectionState as RTCIceConnectionState
  }

  get iceGatheringState(): 'new' | 'gathering' | 'complete' {
    return 'new'
  }

  get signalingState(): 'stable' | 'closed' {
    return this.connectionState === 'closed' ? 'closed' : 'stable'
  }
}

export type { RTCConfiguration, RTCOfferOptions } from './types'
