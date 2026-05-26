import { EventEmitter } from 'events'

import type { JsMediaStreamTrack as NativeMediaStreamTrack } from '@node-webrtc-rust/bindings'

import { MediaStreamTrack } from './MediaStreamTrack'

export class MediaStream extends EventEmitter {
  readonly id: string
  private readonly tracks: MediaStreamTrack[] = []

  constructor(tracks?: MediaStreamTrack[], id = cryptoRandomId()) {
    super()
    this.id = id
    if (tracks) {
      for (const track of tracks) {
        this.addTrack(track)
      }
    }
  }

  static fromNativeTrack(native: NativeMediaStreamTrack): MediaStream {
    const stream = new MediaStream()
    stream.addTrack(new MediaStreamTrack(native))
    return stream
  }

  getTracks(): MediaStreamTrack[] {
    return [...this.tracks]
  }

  getAudioTracks(): MediaStreamTrack[] {
    return this.tracks.filter((track) => track.kind === 'audio')
  }

  getVideoTracks(): MediaStreamTrack[] {
    return this.tracks.filter((track) => track.kind === 'video')
  }

  getTrackById(id: string): MediaStreamTrack | null {
    return this.tracks.find((track) => track.id === id) ?? null
  }

  addTrack(track: MediaStreamTrack): void {
    if (!this.tracks.includes(track)) {
      this.tracks.push(track)
      this.emit('addtrack', { track })
    }
  }

  removeTrack(track: MediaStreamTrack): void {
    const index = this.tracks.indexOf(track)
    if (index >= 0) {
      this.tracks.splice(index, 1)
      this.emit('removetrack', { track })
    }
  }

  clone(): MediaStream {
    return new MediaStream(
      this.tracks.map((track) => track.clone()),
      cryptoRandomId(),
    )
  }

  get active(): boolean {
    return this.tracks.some((track) => track.readyState === 'live')
  }
}

function cryptoRandomId(): string {
  return `stream-${Math.random().toString(36).slice(2, 11)}`
}
