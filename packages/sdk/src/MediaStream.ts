import { EventEmitter } from 'events'

import type { JsMediaStreamTrack as NativeMediaStreamTrack } from '@node-webrtc-rust/bindings'

import { MediaStreamTrack } from './MediaStreamTrack'

/**
 * A collection of media tracks sharing a stream identifier.
 *
 * Remote tracks received via {@link RTCPeerConnection.ontrack} are grouped into
 * a `MediaStream` automatically by the SDK.
 */
export class MediaStream extends EventEmitter {
  /** Unique stream id (matches the sender's track stream id when remote). */
  readonly id: string
  private readonly tracks: MediaStreamTrack[] = []

  /**
   * @param tracks - Optional initial tracks to include.
   * @param id - Stream id; a random id is generated when omitted.
   */
  constructor(tracks?: MediaStreamTrack[], id = cryptoRandomId()) {
    super()
    this.id = id
    if (tracks) {
      for (const track of tracks) {
        this.addTrack(track)
      }
    }
  }

  /** Builds a stream wrapper from a native remote track. */
  static fromNativeTrack(native: NativeMediaStreamTrack): MediaStream {
    const stream = new MediaStream()
    stream.addTrack(new MediaStreamTrack(native))
    return stream
  }

  /** Returns a shallow copy of all tracks in this stream. */
  getTracks(): MediaStreamTrack[] {
    return [...this.tracks]
  }

  /** Returns audio tracks only. */
  getAudioTracks(): MediaStreamTrack[] {
    return this.tracks.filter((track) => track.kind === 'audio')
  }

  /** Returns video tracks only. */
  getVideoTracks(): MediaStreamTrack[] {
    return this.tracks.filter((track) => track.kind === 'video')
  }

  /** Finds a track by id, or null if not present. */
  getTrackById(id: string): MediaStreamTrack | null {
    return this.tracks.find((track) => track.id === id) ?? null
  }

  /** Adds a track if it is not already in the stream. */
  addTrack(track: MediaStreamTrack): void {
    if (!this.tracks.includes(track)) {
      this.tracks.push(track)
      this.emit('addtrack', { track })
    }
  }

  /** Removes a track from the stream. */
  removeTrack(track: MediaStreamTrack): void {
    const index = this.tracks.indexOf(track)
    if (index >= 0) {
      this.tracks.splice(index, 1)
      this.emit('removetrack', { track })
    }
  }

  /** Returns a new stream with cloned track references. */
  clone(): MediaStream {
    return new MediaStream(
      this.tracks.map((track) => track.clone()),
      cryptoRandomId(),
    )
  }

  /** True while at least one contained track is live. */
  get active(): boolean {
    return this.tracks.some((track) => track.readyState === 'live')
  }
}

function cryptoRandomId(): string {
  return `stream-${Math.random().toString(36).slice(2, 11)}`
}
