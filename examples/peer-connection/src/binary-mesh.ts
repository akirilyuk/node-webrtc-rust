/**
 * Three Node peers — mesh binary game-state sync (multiplayer pattern).
 *
 * Run: npm run start:binary-mesh --workspace=@node-webrtc-rust/example-peer-connection
 */

import { RTCPeerConnection, type RTCDataChannel } from '@node-webrtc-rust/sdk'
import { SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import {
  createStateBuffer,
  decodePlayerState,
  encodePlayerState,
  GAME_SYNC_CHANNEL_LABEL,
} from '../../shared/game-state-sync.js'
import { DEMO_ICE_SERVERS, waitForDataChannelOpen } from '../../shared/webrtc-demo-helpers.js'

const TICKS = 10
const TICK_MS = 100
const WALL_MS = 45_000

interface MeshPeer {
  id: string
  signaling: SignalingClient
  links: Map<string, RTCPeerConnection>
  channels: Map<string, RTCDataChannel>
  pendingIce: Map<string, RTCIceCandidateInit[]>
  state: ReturnType<typeof createStateBuffer>
  seenFrom: Set<string>
}

function peerNum(id: string): number {
  return Number(id.replace('pc', ''))
}

async function flushPendingIce(peer: MeshPeer, remoteId: string): Promise<void> {
  const link = peer.links.get(remoteId)
  const pending = peer.pendingIce.get(remoteId) ?? []
  peer.pendingIce.set(remoteId, [])
  if (!link) return
  for (const candidate of pending) {
    await link.addIceCandidate(candidate)
  }
}

function wireChannel(peer: MeshPeer, remoteId: string, channel: RTCDataChannel): void {
  peer.channels.set(remoteId, channel)
  channel.binaryType = 'arraybuffer'
  channel.onmessage = (event) => {
    if (typeof event.data === 'string') return
    const buf = Buffer.isBuffer(event.data) ? event.data : Buffer.from(event.data as ArrayBuffer)
    const decoded = decodePlayerState(buf)
    peer.seenFrom.add(`${remoteId}:tick${decoded.tick}`)
  }
}

async function connectToPeer(peer: MeshPeer, remoteId: string, createOffer: boolean): Promise<void> {
  if (peer.links.has(remoteId)) return

  const link = new RTCPeerConnection({ iceServers: DEMO_ICE_SERVERS })
  peer.links.set(remoteId, link)
  peer.pendingIce.set(remoteId, [])

  if (createOffer) {
    const dc = link.createDataChannel(GAME_SYNC_CHANNEL_LABEL)
    wireChannel(peer, remoteId, dc)
  } else {
    link.ondatachannel = (event) => wireChannel(peer, remoteId, event.channel)
  }

  link.onicecandidate = (event) => {
    if (event.candidate) {
      peer.signaling.sendIceCandidate(remoteId, event.candidate.toJSON())
    }
  }

  if (createOffer) {
    const offer = await link.createOffer()
    await link.setLocalDescription(offer)
    await link.gatheringComplete()
    peer.signaling.sendOffer(remoteId, link.localDescription!.toJSON())
  }
}

async function onOffer(peer: MeshPeer, fromPeerId: string, sdp: RTCSessionDescriptionInit): Promise<void> {
  if (!peer.links.has(fromPeerId)) {
    await connectToPeer(peer, fromPeerId, false)
  }
  const link = peer.links.get(fromPeerId)
  if (!link) return
  await link.setRemoteDescription(sdp)
  await flushPendingIce(peer, fromPeerId)
  const answer = await link.createAnswer()
  await link.setLocalDescription(answer)
  await link.gatheringComplete()
  peer.signaling.sendAnswer(fromPeerId, link.localDescription!.toJSON())
}

async function onAnswer(peer: MeshPeer, fromPeerId: string, sdp: RTCSessionDescriptionInit): Promise<void> {
  const link = peer.links.get(fromPeerId)
  if (!link) return
  await link.setRemoteDescription(sdp)
  await flushPendingIce(peer, fromPeerId)
}

async function onRemoteIce(
  peer: MeshPeer,
  fromPeerId: string,
  candidate: RTCIceCandidateInit,
): Promise<void> {
  const link = peer.links.get(fromPeerId)
  if (!link || !candidate.candidate) return
  if (!link.remoteDescription) {
    peer.pendingIce.get(fromPeerId)?.push(candidate)
    return
  }
  await link.addIceCandidate(candidate)
}

function createMeshPeer(id: string, url: string, room: string): MeshPeer {
  const signaling = new SignalingClient({ url, room, peerId: id })
  const peer: MeshPeer = {
    id,
    signaling,
    links: new Map(),
    channels: new Map(),
    pendingIce: new Map(),
    state: createStateBuffer(),
    seenFrom: new Set(),
  }

  signaling.on('offer', ({ peerId, sdp }) => {
    void onOffer(peer, peerId, sdp).catch(console.error)
  })

  signaling.on('answer', ({ peerId, sdp }) => {
    void onAnswer(peer, peerId, sdp).catch(console.error)
  })

  signaling.on('ice-candidate', ({ peerId, candidate }) => {
    void onRemoteIce(peer, peerId, candidate).catch(console.error)
  })

  return peer
}

async function main(): Promise<void> {
  const wall = setTimeout(() => {
    console.error('binary-mesh wall clock timeout')
    process.exit(1)
  }, WALL_MS)

  const server = new SignalingServer({ port: 0 })
  await server.listen(0)
  const room = 'binary-mesh'
  const url = `ws://localhost:${server.port}`

  const peers = ['pc1', 'pc2', 'pc3'].map((id) => createMeshPeer(id, url, room))
  for (const peer of peers) {
    await peer.signaling.connect()
  }

  const peerIds = peers.map((p) => p.id)
  for (const peer of peers) {
    for (const remoteId of peerIds) {
      if (remoteId === peer.id) continue
      if (peerNum(peer.id) > peerNum(remoteId)) {
        await connectToPeer(peer, remoteId, true)
      }
    }
  }

  const expectedLinks = peers.length * (peers.length - 1)
  const deadline = Date.now() + 20_000
  while (Date.now() < deadline) {
    const channels = peers.flatMap((p) => [...p.channels.values()])
    if (
      channels.length === expectedLinks &&
      channels.every((ch) => ch.readyState === 'open')
    ) {
      break
    }
    await new Promise((r) => setTimeout(r, 50))
  }

  const channels = peers.flatMap((p) => [...p.channels.values()])
  if (channels.length !== expectedLinks || channels.some((ch) => ch.readyState !== 'open')) {
    throw new Error(
      `mesh links incomplete: ${channels.length}/${expectedLinks} channels, states=${channels.map((c) => c.readyState).join(',')}`,
    )
  }

  for (const channel of channels) {
    await waitForDataChannelOpen(channel)
  }

  for (let tick = 1; tick <= TICKS; tick++) {
    for (const peer of peers) {
      encodePlayerState(peer.state.view, 0, {
        tick,
        playerId: peerNum(peer.id),
        x: tick * 0.25,
        y: peerNum(peer.id),
        rot: 0,
      })
      for (const channel of peer.channels.values()) {
        channel.send(peer.state.bytes)
      }
    }
    await new Promise((r) => setTimeout(r, TICK_MS))
  }

  let ok = true
  for (const peer of peers) {
    for (const remote of peers) {
      if (remote.id === peer.id) continue
      for (let tick = 1; tick <= TICKS; tick++) {
        const key = `${remote.id}:tick${tick}`
        if (!peer.seenFrom.has(key)) {
          console.error(`${peer.id} missing ${key}`)
          ok = false
        }
      }
    }
  }

  for (const peer of peers) {
    for (const link of peer.links.values()) link.close()
    peer.signaling.disconnect()
  }
  await server.close()
  clearTimeout(wall)

  if (!ok) throw new Error('mesh binary sync incomplete')
  console.log(`Mesh binary sync OK (${peers.length} peers, ${TICKS} ticks each)`)
  process.exit(0)
}

void main().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
