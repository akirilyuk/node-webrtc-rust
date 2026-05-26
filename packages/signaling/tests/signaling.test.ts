import { describe, expect, test } from 'vitest'

import { SignalingClient, SignalingServer } from '../src'

describe('SignalingServer', () => {
  test('relays join notifications between peers', async () => {
    const server = new SignalingServer({ port: 0 })
    await server.listen(0)

    const clientA = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'test',
      peerId: 'a',
    })
    const clientB = new SignalingClient({
      url: `ws://localhost:${server.port}`,
      room: 'test',
      peerId: 'b',
    })

    const joined = new Promise<string>((resolve) => {
      clientB.once('peer-joined', resolve)
    })

    await clientA.connect()
    await clientB.connect()

    await expect(joined).resolves.toBe('a')

    clientA.disconnect()
    clientB.disconnect()
    await server.close()
  })
})
