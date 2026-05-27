import { describe, expect, test } from 'vitest'

import { RTCPeerConnection } from '../src'
import { waitForConnection, waitForMessage, waitForOpen } from './helpers'

const turnAvailable = process.env.TURN_AVAILABLE === '1'
const turnHost = process.env.TURN_URL ?? 'turn:localhost:3478'

const turnConfig = {
  iceServers: [
    {
      urls: turnHost,
      username: 'testuser',
      credential: 'testpass',
    },
  ],
  iceTransportPolicy: 'relay' as const,
}

async function exchangeSdp(pc1: RTCPeerConnection, pc2: RTCPeerConnection): Promise<void> {
  const dc1 = pc1.createDataChannel('turn-test')

  const offer = await pc1.createOffer()
  await pc1.setLocalDescription(offer)
  await pc1.gatheringComplete()

  await pc2.setRemoteDescription(pc1.localDescription!)
  const answer = await pc2.createAnswer()
  await pc2.setLocalDescription(answer)
  await pc2.gatheringComplete()

  await pc1.setRemoteDescription(pc2.localDescription!)

  const dc2Promise = new Promise<typeof dc1>((resolve) => {
    pc2.ondatachannel = (event) => resolve(event.channel)
  })

  const dc2 = await dc2Promise
  await waitForOpen(dc1)
  await waitForOpen(dc2)
  await waitForConnection(pc1)
  await waitForConnection(pc2)

  dc1.send('turn relay works')
  const message = await waitForMessage(dc2)
  expect(message.data).toBe('turn relay works')
}

describe.skipIf(!turnAvailable)('TURN relay integration', () => {
  test('peers connect via TURN relay and exchange DataChannel messages', async () => {
    const pc1 = new RTCPeerConnection(turnConfig)
    const pc2 = new RTCPeerConnection(turnConfig)

    try {
      await exchangeSdp(pc1, pc2)
    } finally {
      pc1.close()
      pc2.close()
    }
  })
})
