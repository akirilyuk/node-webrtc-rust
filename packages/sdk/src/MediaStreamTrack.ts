import { EventEmitter } from 'events'

import type {
  JsLocalAudioTrack,
  JsMediaStreamTrack as NativeMediaStreamTrack,
} from '@node-webrtc-rust/bindings'

import type { TrackKind } from './types'

type NativeTrack = NativeMediaStreamTrack | JsLocalAudioTrack

/**
 * A single audio or video track within a {@link MediaStream}.
 *
 * Represents both local tracks (see {@link LocalAudioTrack}) and remote tracks
 * received through {@link RTCPeerConnection.ontrack} after the sender writes
 * the first PCM frame with {@link LocalAudioTrack.writeSample}.
 */
export class MediaStreamTrack extends EventEmitter {
  /** Unique track identifier. */
  readonly id: string
  /** `audio` or `video`. */
  readonly kind: TrackKind
  /** Human-readable label (defaults to the track id). */
  readonly label: string
  /** When false, the track is muted for transmission/rendering. */
  enabled: boolean
  /** `live` until {@link stop} is called. */
  readonly readyState: 'live' | 'ended' = 'live'

  constructor(native: NativeTrack) {
    super()
    this.id = native.id
    this.kind = native.kind === 'video' ? 'video' : 'audio'
    this.label = native.id
    this.enabled = native.enabled
  }

  /** Marks the track ended locally. */
  stop(): void {
    ;(this as { readyState: 'live' | 'ended' }).readyState = 'ended'
  }

  /** Returns this track (shallow clone; shared native handle). */
  clone(): MediaStreamTrack {
    return this
  }
}

export type { TrackKind } from './types'
