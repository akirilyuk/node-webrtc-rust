import { EventEmitter } from 'events'

import {
  JsPeerConnection as NativePeerConnection,
  type JsRtcIceCandidate,
  type JsRtcSessionDescription,
} from '@node-webrtc-rust/bindings'

import { debugEvent, debugFn, setDebugEnabled } from './debug'
import { MediaStream } from './MediaStream'
import { MediaStreamTrack } from './MediaStreamTrack'
import type { LocalAudioTrack } from './LocalAudioTrack'
import { RTCDataChannel } from './RTCDataChannel'
import { RTCIceCandidate } from './RTCIceCandidate'
import { RTCRtpSender } from './RTCRtpSender'
import { RTCSessionDescription } from './RTCSessionDescription'
import type {
  RTCConfiguration,
  RTCDataChannelEvent,
  RTCDataChannelInit,
  RTCIceConnectionState,
  RTCIceCandidateInit,
  RTCIceGatheringState,
  RTCPeerConnectionIceEvent,
  RTCPeerConnectionState,
  RTCSignalingState,
  RTCOfferOptions,
  RTCTrackEvent,
} from './types'

function toNativeConfig(config?: RTCConfiguration) {
  if (!config) return undefined
  if (config.debug !== undefined) {
    setDebugEnabled(config.debug)
  }
  return {
    iceServers: config.iceServers?.map((server) => ({
      urls: Array.isArray(server.urls) ? server.urls : [server.urls],
      username: server.username,
      credential: server.credential,
      credentialType: server.credentialType,
    })),
    iceTransportPolicy: config.iceTransportPolicy,
    debug: config.debug,
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

/**
 * Browser-compatible WebRTC peer connection for Node.js.
 *
 * Wraps the native Rust engine and exposes the familiar W3C `RTCPeerConnection`
 * event-handler API. Use with a signaling layer (see `@node-webrtc-rust/signaling`)
 * to exchange SDP and ICE candidates between peers.
 */
export class RTCPeerConnection extends EventEmitter {
  private readonly native: NativePeerConnection
  private _localDescription: RTCSessionDescription | null = null
  private _remoteDescription: RTCSessionDescription | null = null

  /** Fired when a local ICE candidate is gathered; `candidate` is null when gathering completes. */
  onicecandidate: ((event: RTCPeerConnectionIceEvent) => void) | null = null
  /**
   * Fired when a remote media track is received.
   *
   * For audio, this handler runs only after the remote peer sends at least one
   * PCM frame via {@link LocalAudioTrack.writeSample} — adding a track with
   * {@link addTrack} and completing ICE/SDP negotiation alone is not enough.
   */
  ontrack: ((event: RTCTrackEvent) => void) | null = null
  /** Fired when the remote peer opens a data channel. */
  ondatachannel: ((event: RTCDataChannelEvent) => void) | null = null
  /** Fired when {@link connectionState} changes. */
  onconnectionstatechange: ((event: Event) => void) | null = null
  /** Fired when {@link iceConnectionState} changes. */
  oniceconnectionstatechange: ((event: Event) => void) | null = null
  /** Fired when negotiation is required (e.g. after {@link addTrack}). */
  onnegotiationneeded: ((event: Event) => void) | null = null

  /**
   * Creates a peer connection.
   * @param config - Optional ICE servers and transport policy (STUN/TURN).
   */
  constructor(config?: RTCConfiguration) {
    super()
    debugFn('sdk::RTCPeerConnection', 'constructor', config?.debug !== undefined ? `debug=${config.debug}` : '')
    this.native = new NativePeerConnection(toNativeConfig(config))

    this.native.setOnIceCandidate((_err, candidate) => {
      debugEvent('sdk::RTCPeerConnection', 'icecandidate', candidate ? 'present' : 'null')
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

    this.native.setOnTrack((_err, track) => {
      if (!track) return
      debugEvent('sdk::RTCPeerConnection', 'track', `id=${track.id}`)
      const wrappedTrack = new MediaStreamTrack(track)
      const stream = MediaStream.fromNativeTrack(track)
      const event: RTCTrackEvent = { track: wrappedTrack, streams: [stream] }
      this.ontrack?.(event)
      this.emit('track', event)
    })

    this.native.setOnDataChannel((_err, channel) => {
      if (!channel) return
      debugEvent('sdk::RTCPeerConnection', 'datachannel', `label=${channel.label}`)
      const wrapped = new RTCDataChannel(channel)
      const event: RTCDataChannelEvent = { channel: wrapped }
      this.ondatachannel?.(event)
      this.emit('datachannel', event)
    })

    this.native.setOnConnectionStateChange((_err, state) => {
      debugEvent('sdk::RTCPeerConnection', 'connectionstatechange', String(state))
      const event = new Event('connectionstatechange')
      this.onconnectionstatechange?.(event)
      this.emit('connectionstatechange', event)
    })

    this.native.setOnIceConnectionStateChange((_err, state) => {
      debugEvent('sdk::RTCPeerConnection', 'iceconnectionstatechange', String(state))
      const event = new Event('iceconnectionstatechange')
      this.oniceconnectionstatechange?.(event)
      this.emit('iceconnectionstatechange', event)
    })

    this.native.setOnNegotiationNeeded((_err) => {
      debugEvent('sdk::RTCPeerConnection', 'negotiationneeded')
      const event = new Event('negotiationneeded')
      this.onnegotiationneeded?.(event)
      this.emit('negotiationneeded', event)
    })
  }

  /**
   * Creates an SDP offer describing the local media and data channel setup.
   * @param _options - Reserved for future W3C offer options.
   */
  async createOffer(_options?: RTCOfferOptions): Promise<RTCSessionDescription> {
    debugFn('sdk::RTCPeerConnection', 'createOffer')
    void _options
    return fromNativeDescription(await this.native.createOffer())
  }

  /** Creates an SDP answer after a remote offer has been applied via {@link setRemoteDescription}. */
  async createAnswer(): Promise<RTCSessionDescription> {
    debugFn('sdk::RTCPeerConnection', 'createAnswer')
    return fromNativeDescription(await this.native.createAnswer())
  }

  /**
   * Applies the local session description (offer or answer).
   * Triggers ICE gathering for offers and answers.
   */
  async setLocalDescription(desc: RTCSessionDescription): Promise<void> {
    debugFn('sdk::RTCPeerConnection', 'setLocalDescription', `type=${desc.type}`)
    await this.native.setLocalDescription(toNativeDescription(desc))
    this._localDescription = desc
  }

  private async refreshLocalDescription(): Promise<void> {
    const local = await this.native.localDescription()
    if (local) {
      this._localDescription = fromNativeDescription(local)
    }
  }

  /** Applies the remote session description received from the peer via signaling. */
  async setRemoteDescription(desc: RTCSessionDescription): Promise<void> {
    debugFn('sdk::RTCPeerConnection', 'setRemoteDescription', `type=${desc.type}`)
    await this.native.setRemoteDescription(toNativeDescription(desc))
    this._remoteDescription = desc
  }

  /**
   * Adds a trickle ICE candidate from the remote peer.
   * @param candidate - Candidate string and metadata from signaling.
   */
  async addIceCandidate(candidate: RTCIceCandidate | RTCIceCandidateInit): Promise<void> {
    debugFn('sdk::RTCPeerConnection', 'addIceCandidate')
    const init = candidate instanceof RTCIceCandidate ? candidate.toJSON() : candidate
    const nativeCandidate: JsRtcIceCandidate = {
      candidate: init.candidate ?? '',
      sdpMid: init.sdpMid ?? undefined,
      sdpMLineIndex: init.sdpMLineIndex ?? undefined,
      usernameFragment: init.usernameFragment ?? undefined,
    }
    await this.native.addIceCandidate(nativeCandidate)
  }

  /**
   * Creates an outgoing data channel on this connection.
   * @param label - Application-defined channel name.
   * @param options - Ordering, reliability, and negotiated channel id.
   */
  createDataChannel(label: string, options?: RTCDataChannelInit): RTCDataChannel {
    debugFn('sdk::RTCPeerConnection', 'createDataChannel', `label=${label}`)
    return RTCDataChannel.fromNativePromise(
      this.native.createDataChannel(label, options),
      label,
      options,
    )
  }

  /**
   * Adds a local audio track for sending to the remote peer.
   *
   * The remote peer's {@link ontrack} handler fires only after the first
   * {@link LocalAudioTrack.writeSample} call delivers RTP — call it once the
   * connection is established (and keep calling it to stream audio).
   *
   * @returns An {@link RTCRtpSender} handle for the added track.
   */
  async addTrack(track: LocalAudioTrack): Promise<RTCRtpSender> {
    debugFn('sdk::RTCPeerConnection', 'addTrack', `id=${track.id}`)
    const sender = await this.native.addTrack(track.native)
    return new RTCRtpSender(sender, track)
  }

  /**
   * Blocks until ICE gathering completes and refreshes {@link localDescription}
   * with gathered candidates. Call after {@link setLocalDescription} before sending SDP.
   */
  async gatheringComplete(): Promise<void> {
    debugFn('sdk::RTCPeerConnection', 'gatheringComplete')
    await this.native.gatheringComplete()
    await this.refreshLocalDescription()
  }

  /** Closes the connection and releases native resources. */
  close(): void {
    debugFn('sdk::RTCPeerConnection', 'close')
    void this.native.close()
  }

  /** Last local description set or refreshed after gathering. */
  get localDescription(): RTCSessionDescription | null {
    return this._localDescription
  }

  /** Last remote description applied via {@link setRemoteDescription}. */
  get remoteDescription(): RTCSessionDescription | null {
    return this._remoteDescription
  }

  /** Current overall connection state. */
  get connectionState(): RTCPeerConnectionState {
    return this.native.connectionState as RTCPeerConnectionState
  }

  /** Current ICE transport state. */
  get iceConnectionState(): RTCIceConnectionState {
    return this.native.iceConnectionState as RTCIceConnectionState
  }

  /** Current ICE candidate gathering state. */
  get iceGatheringState(): RTCIceGatheringState {
    return this.native.iceGatheringState as RTCIceGatheringState
  }

  /** Current SDP negotiation state. */
  get signalingState(): RTCSignalingState {
    return this.native.signalingState as RTCSignalingState
  }
}

export type { RTCConfiguration, RTCOfferOptions } from './types'
