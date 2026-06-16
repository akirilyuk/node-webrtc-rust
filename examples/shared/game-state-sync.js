/** Browser ESM mirror of game-state-sync.ts for /shared/ static imports. */

export const GAME_SYNC_CHANNEL_LABEL = 'game-sync'
export const PLAYER_STATE_FRAME_BYTES = 20

export function createStateBuffer() {
  const buffer = new ArrayBuffer(PLAYER_STATE_FRAME_BYTES)
  const view = new DataView(buffer)
  const bytes = new Uint8Array(buffer)
  return { buffer, view, bytes }
}

export function encodePlayerState(view, offset, state) {
  view.setUint32(offset, state.tick >>> 0, true)
  view.setUint16(offset + 4, state.playerId & 0xffff, true)
  view.setUint16(offset + 6, 0, true)
  view.setFloat32(offset + 8, state.x, true)
  view.setFloat32(offset + 12, state.y, true)
  view.setFloat32(offset + 16, state.rot, true)
}

export function decodePlayerState(source, offset = 0) {
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
