import { describe, expect, test } from 'vitest'

import { ConferenceServer } from '../src/conference'

describe('ConferenceServer', () => {
  test('creates and lists rooms', async () => {
    const server = new ConferenceServer()
    const room = await server.createRoom('test-room', { maxParticipants: 4 })

    expect(await server.listRooms()).toContain('test-room')
    expect(await room.isMixingEnabled()).toBe(true)
    expect(await room.listParticipants()).toEqual([])

    await server.destroyRoom('test-room')
    expect(await server.listRooms()).not.toContain('test-room')
  })

  test('getRoom returns cached handle', async () => {
    const server = new ConferenceServer()
    const created = await server.createRoom('cached')
    const fetched = await server.getRoom('cached')

    expect(fetched).toBeDefined()
    expect(fetched).toBe(created)

    await server.destroyRoom('cached')
  })

  test('setMixingEnabled toggles room-wide mixing', async () => {
    const server = new ConferenceServer()
    const room = await server.createRoom('mix-toggle')

    await room.setMixingEnabled(false)
    expect(await room.isMixingEnabled()).toBe(false)

    await room.setMixingEnabled(true)
    expect(await room.isMixingEnabled()).toBe(true)

    await server.destroyRoom('mix-toggle')
  })

  test('handleSignalingMessage returns offer JSON on join', async () => {
    const server = new ConferenceServer()
    await server.createRoom('sig-room')

    const room = await server.getRoom('sig-room')
    expect(room).toBeDefined()

    const response = await room!.handleSignalingMessage(
      JSON.stringify({
        type: 'join',
        participantId: 'alice',
        roomId: 'sig-room',
      }),
    )

    const responses = JSON.parse(response) as Array<{
      type?: string
      sdp?: string
      participantId?: string
    }>
    expect(responses).toHaveLength(1)
    expect(responses[0]?.type).toBe('offer')
    expect(responses[0]?.participantId).toBe('alice')
    expect(typeof responses[0]?.sdp).toBe('string')
    expect(responses[0]!.sdp!.length).toBeGreaterThan(0)

    await server.destroyRoom('sig-room')
  })

  test('muteParticipant rejects listener scope without listenerId', async () => {
    const server = new ConferenceServer()
    const room = await server.createRoom('mute-room')

    await room.handleSignalingMessage(
      JSON.stringify({ type: 'join', participantId: 'alice', roomId: 'mute-room' }),
    )

    await expect(
      room.muteParticipant('alice', { scope: 'listener' }),
    ).rejects.toThrow(/listenerId/i)

    await server.destroyRoom('mute-room')
  })
})
