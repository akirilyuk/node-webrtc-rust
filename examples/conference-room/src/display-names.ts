/**
 * In-memory display-name registry for conference room examples.
 * Maps signaling peer ids to human-readable names for UI and logs.
 */

export class DisplayNameRegistry {
  private readonly rooms = new Map<string, Map<string, string>>()

  /** Registers or updates a peer's display name in a room. */
  set(roomId: string, peerId: string, displayName: string): void {
    const trimmed = displayName.trim().slice(0, 32)
    if (!trimmed) {
      return
    }

    let room = this.rooms.get(roomId)
    if (!room) {
      room = new Map()
      this.rooms.set(roomId, room)
    }
    room.set(peerId, trimmed)
  }

  /** Removes names for peers no longer in the conference participant list. */
  prune(roomId: string, activePeerIds: readonly string[]): void {
    const room = this.rooms.get(roomId)
    if (!room) {
      return
    }
    const active = new Set(activePeerIds)
    for (const id of room.keys()) {
      if (!active.has(id)) {
        room.delete(id)
      }
    }
  }

  lookup(roomId: string, peerId: string): string | undefined {
    return this.rooms.get(roomId)?.get(peerId)
  }

  /** Adds `displayName` to each participant row for the browser UI. */
  enrich<T extends { id: string; connectionState: string }>(
    roomId: string,
    participants: T[],
  ): Array<T & { displayName: string }> {
    this.prune(
      roomId,
      participants.map((p) => p.id),
    )
    return participants.map((p) => ({
      ...p,
      displayName: this.lookup(roomId, p.id) ?? 'Unknown',
    }))
  }
}
