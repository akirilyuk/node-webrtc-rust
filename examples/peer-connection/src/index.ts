import { RTCPeerConnection } from '@node-webrtc-rust/sdk'
import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

async function main(): Promise<void> {
  const server = new SignalingServer({ port: 8080 })
  await server.listen()

  const pc1 = new RTCPeerConnection({
    iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
  })
  const pc2 = new RTCPeerConnection({
    iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
  })

  const sig1 = new SignalingClient({ url: `ws://localhost:${server.port}`, room: 'demo' })
  const sig2 = new SignalingClient({ url: `ws://localhost:${server.port}`, room: 'demo' })

  autoNegotiate({ pc: pc1, signaling: sig1, polite: true })
  autoNegotiate({ pc: pc2, signaling: sig2, polite: false })

  await sig1.connect()
  await sig2.connect()

  const dc = pc1.createDataChannel('chat')
  dc.onopen = () => {
    dc.send('Hello from peer 1!')
  }

  pc2.ondatachannel = (event) => {
    event.channel.onmessage = (message) => {
      console.log('Received:', message.data)
      pc1.close()
      pc2.close()
      void server.close()
    }
  }
}

void main()
