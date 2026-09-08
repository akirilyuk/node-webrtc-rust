export type ClipStatus = 'buffering' | 'playing' | 'stopped' | 'ended' | 'error'

export interface ClipPlayerStatus {
  playId: string
  status: ClipStatus
  positionMs: number
  durationMs?: number
  bufferedMs: number
  error?: string
}

export interface ProgressiveClipWriter {
  append(chunk: Uint8Array | Buffer): void
  markEof(): void
  isEof(): boolean
}

export interface ProgressiveClip {
  playId: string
  writer: ProgressiveClipWriter
}
