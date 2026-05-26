import { EventEmitter } from 'events'

import type {
  JsLocalAudioTrack,
  JsMediaStreamTrack as NativeMediaStreamTrack,
} from '@node-webrtc-rust/bindings'

import type { TrackKind } from './types'

type NativeTrack = NativeMediaStreamTrack | JsLocalAudioTrack

export class MediaStreamTrack extends EventEmitter {
  readonly id: string
  readonly kind: TrackKind
  readonly label: string
  enabled: boolean
  readonly readyState: 'live' | 'ended' = 'live'

  constructor(native: NativeTrack) {
    super()
    this.id = native.id
    this.kind = native.kind === 'video' ? 'video' : 'audio'
    this.label = native.id
    this.enabled = native.enabled
  }

  stop(): void {
    ;(this as { readyState: 'live' | 'ended' }).readyState = 'ended'
  }

  clone(): MediaStreamTrack {
    return this
  }
}

export type { TrackKind } from './types'
