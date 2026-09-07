import {
  getClipStatus as nativeGetClipStatus,
  playClipFromBytes as nativePlayClipFromBytes,
  playClipFromPath as nativePlayClipFromPath,
  playClipProgressive as nativePlayClipProgressive,
  stopClip as nativeStopClip,
  type JsClipPlayerStatus,
  type JsClipStatus,
  type JsGrowingClipWriter,
} from '@node-webrtc-rust/bindings'
import { Buffer } from 'node:buffer'

import type { ClipPlayerStatus, ClipStatus, ProgressiveClip, ProgressiveClipWriter } from './types.js'

function mapStatus(status: JsClipStatus): ClipStatus {
  switch (status) {
    case 'Buffering':
      return 'buffering'
    case 'Playing':
      return 'playing'
    case 'Stopped':
      return 'stopped'
    case 'Ended':
      return 'ended'
    case 'Error':
      return 'error'
    default:
      return 'error'
  }
}

function fromNativeStatus(status: JsClipPlayerStatus): ClipPlayerStatus {
  return {
    playId: status.playId,
    status: mapStatus(status.status),
    positionMs: status.positionMs,
    durationMs: status.durationMs ?? undefined,
    bufferedMs: status.bufferedMs,
    error: status.error ?? undefined,
  }
}

function toBuffer(data: Uint8Array | Buffer): Buffer {
  return Buffer.isBuffer(data) ? data : Buffer.from(data)
}

function wrapWriter(writer: JsGrowingClipWriter): ProgressiveClipWriter {
  return {
    append(chunk: Uint8Array | Buffer) {
      writer.append(toBuffer(chunk))
    },
    markEof() {
      writer.markEof()
    },
    isEof() {
      return writer.isEof()
    },
  }
}

/** Start clip playback from a filesystem path. */
export function playClip(path: string): string {
  return nativePlayClipFromPath(path)
}

/** Start clip playback from an in-memory encoded buffer. */
export function playClipBytes(data: Uint8Array | Buffer): string {
  return nativePlayClipFromBytes(toBuffer(data))
}

/** Start progressive clip playback; append bytes via the returned writer. */
export function playClipProgressive(): ProgressiveClip {
  const writer = nativePlayClipProgressive()
  return {
    playId: writer.playId,
    writer: wrapWriter(writer),
  }
}

/** Query status for a play session. */
export function getClip(playId: string): ClipPlayerStatus | undefined {
  const status = nativeGetClipStatus(playId)
  return status ? fromNativeStatus(status) : undefined
}

/** Stop playback for a play session. */
export function stopClip(playId: string): boolean {
  return nativeStopClip(playId)
}

export type { ClipPlayerStatus, ClipStatus, ProgressiveClip, ProgressiveClipWriter } from './types.js'
