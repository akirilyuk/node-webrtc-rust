/**
 * Fixed-size binary player state frames for multiplayer DataChannel demos.
 *
 * Layout (little-endian, 20 bytes):
 * - u32 tick
 * - u16 playerId
 * - u16 reserved
 * - f32 x
 * - f32 y
 * - f32 rot
 *
 * Reuse {@link createStateBuffer} and mutate via `view` each tick — send `bytes`
 * without JSON or per-frame allocations.
 */

export const GAME_SYNC_CHANNEL_LABEL = 'game-sync'
export const PLAYER_STATE_FRAME_BYTES = 20

export interface PlayerState {
  tick: number
  playerId: number
  x: number
  y: number
  rot: number
}

export interface StateBuffer {
  buffer: ArrayBuffer
  view: DataView
  /** Send this view on the data channel each tick. */
  bytes: Uint8Array
}

export function createStateBuffer(): StateBuffer {
  const buffer = new ArrayBuffer(PLAYER_STATE_FRAME_BYTES)
  const view = new DataView(buffer)
  const bytes = new Uint8Array(buffer)
  return { buffer, view, bytes }
}

export function encodePlayerState(view: DataView, offset: number, state: PlayerState): void {
  view.setUint32(offset, state.tick >>> 0, true)
  view.setUint16(offset + 4, state.playerId & 0xffff, true)
  view.setUint16(offset + 6, 0, true)
  view.setFloat32(offset + 8, state.x, true)
  view.setFloat32(offset + 12, state.y, true)
  view.setFloat32(offset + 16, state.rot, true)
}

export function decodePlayerState(source: ArrayBuffer | Uint8Array, offset = 0): PlayerState {
  const view =
    source instanceof ArrayBuffer
      ? new DataView(source, offset, PLAYER_STATE_FRAME_BYTES)
      : new DataView(source.buffer, source.byteOffset + offset, PLAYER_STATE_FRAME_BYTES)
  return {
    tick: view.getUint32(0, true),
    playerId: view.getUint16(4, true),
    x: view.getFloat32(8, true),
    y: view.getFloat32(12, true),
    rot: view.getFloat32(16, true),
  }
}
