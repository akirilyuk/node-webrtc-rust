import { SignalingClient } from '@node-webrtc-rust/signaling'

import { CosineRoomHost, SERVER_PEER_ID, type CosineRoomHostOptions } from './cosine-room-host'

export interface RoomManagerOptions {
  signalingUrl: string
  iceServers: { urls: string | string[] }[]
  frequencyHz?: number
  amplitude?: number
}

interface ActiveRoom {
  signaling: SignalingClient
  host: CosineRoomHost
}

/** Lazily spins up a cosine broadcaster and signaling client per chat room. */
export class RoomManager {
  private readonly rooms = new Map<string, ActiveRoom>()
  private readonly options: RoomManagerOptions

  constructor(options: RoomManagerOptions) {
    this.options = options
  }

  async ensureRoom(room: string): Promise<void> {
    if (this.rooms.has(room)) return

    const streamOptions: CosineRoomHostOptions = {
      frequencyHz: this.options.frequencyHz ?? 440,
      amplitude: this.options.amplitude ?? 0.2,
    }

    const signaling = new SignalingClient({
      url: this.options.signalingUrl,
      room,
      peerId: SERVER_PEER_ID,
    })
    await signaling.connect()

    const host = new CosineRoomHost(signaling, streamOptions, this.options.iceServers)
    this.rooms.set(room, { signaling, host })
    console.log(`Room ready: ${room}`)
  }

  async close(): Promise<void> {
    for (const [room, active] of this.rooms) {
      active.host.close()
      active.signaling.disconnect()
      this.rooms.delete(room)
    }
  }
}
