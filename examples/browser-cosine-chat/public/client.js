const SERVER_PEER_ID = 'cosine-server'
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

const displayNameInput = document.getElementById('display-name')
const roomInput = document.getElementById('room')
const connectButton = document.getElementById('connect')
const disconnectButton = document.getElementById('disconnect')
const statusEl = document.getElementById('status')
const audioStatusEl = document.getElementById('audio-status')
const toneEl = document.getElementById('tone')
const messagesEl = document.getElementById('messages')
const chatForm = document.getElementById('chat-form')
const messageInput = document.getElementById('message')

/** @type {WebSocket | null} */
let ws = null
/** @type {string | null} */
let clientId = null
/** @type {string | null} */
let room = null
/** @type {string} */
let displayName = 'Guest'

/** @type {Map<string, { pc: RTCPeerConnection, dc?: RTCDataChannel }>} */
const peerConnections = new Map()
/** @type {Map<string, RTCDataChannel>} */
const chatChannels = new Map()

connectButton.addEventListener('click', () => {
  void connect()
})

disconnectButton.addEventListener('click', () => {
  disconnect()
})

chatForm.addEventListener('submit', (event) => {
  event.preventDefault()
  const text = messageInput.value.trim()
  if (!text) return
  broadcastChat({ type: 'chat', name: displayName, text })
  appendMessage(`${displayName} (you): ${text}`)
  messageInput.value = ''
})

async function connect() {
  if (ws) return

  room = roomInput.value.trim() || 'lobby'
  displayName = displayNameInput.value.trim() || `Guest-${Math.random().toString(36).slice(2, 6)}`
  clientId = `client-${Math.random().toString(36).slice(2, 10)}`

  setStatus(`Joining room "${room}"…`)
  connectButton.disabled = true

  const ensureResponse = await fetch('/api/rooms', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ room }),
  })
  if (!ensureResponse.ok) {
    setStatus('Failed to prepare room on server')
    connectButton.disabled = false
    return
  }

  const wsUrl = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`
  ws = new WebSocket(wsUrl)

  ws.onopen = () => {
    sendSignal({ type: 'join', room, peerId: clientId })
    setStatus(`Connected to room "${room}" as ${displayName}`)
    disconnectButton.disabled = false
    messageInput.disabled = false
    chatForm.querySelector('button').disabled = false
    appendSystem(`Joined room "${room}"`)
  }

  ws.onmessage = (event) => {
    handleSignal(JSON.parse(event.data))
  }

  ws.onclose = () => {
    appendSystem('Signaling disconnected')
    cleanupPeers()
    ws = null
    clientId = null
    connectButton.disabled = false
    disconnectButton.disabled = true
    messageInput.disabled = true
    chatForm.querySelector('button').disabled = true
    setStatus('Disconnected')
  }

  ws.onerror = () => {
    setStatus('WebSocket error')
  }
}

function disconnect() {
  ws?.close()
  cleanupPeers()
  toneEl.srcObject = null
  audioStatusEl.textContent = 'Waiting for server track…'
}

function cleanupPeers() {
  for (const { pc } of peerConnections.values()) {
    pc.close()
  }
  peerConnections.clear()
  chatChannels.clear()
}

function handleSignal(message) {
  switch (message.type) {
    case 'peer-joined':
      onPeerJoined(message.peerId)
      break
    case 'peer-left':
      onPeerLeft(message.peerId)
      break
    case 'offer':
      void onOffer(message.peerId, message.sdp)
      break
    case 'answer':
      void onAnswer(message.peerId, message.sdp)
      break
    case 'ice-candidate':
      void onRemoteIce(message.peerId, message.candidate)
      break
    default:
      break
  }
}

function onPeerJoined(peerId) {
  if (!clientId || peerId === clientId || peerConnections.has(peerId)) return

  if (peerId === SERVER_PEER_ID) {
    appendSystem('Server joined — waiting for audio offer')
    return
  }

  appendSystem(`${peerId} joined the room`)

  // Lexicographic tie-break: higher id creates the chat offer.
  if (clientId > peerId) {
    void connectToPeer(peerId, true)
  }
}

function onPeerLeft(peerId) {
  appendSystem(`${peerId} left the room`)
  const session = peerConnections.get(peerId)
  if (session) {
    session.pc.close()
    peerConnections.delete(peerId)
    chatChannels.delete(peerId)
  }
}

async function connectToPeer(peerId, createOffer) {
  const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS })
  peerConnections.set(peerId, { pc })

  if (createOffer) {
    const dc = pc.createDataChannel('chat')
    wireChatChannel(peerId, dc)
  } else {
    pc.ondatachannel = (event) => wireChatChannel(peerId, event.channel)
  }

  pc.onicecandidate = (event) => {
    if (event.candidate) {
      sendSignal({
        type: 'ice-candidate',
        targetPeerId: peerId,
        candidate: event.candidate.toJSON(),
      })
    }
  }

  if (createOffer) {
    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    await waitGatheringComplete(pc)
    sendSignal({
      type: 'offer',
      targetPeerId: peerId,
      sdp: pc.localDescription,
    })
  }
}

async function onOffer(fromPeerId, sdp) {
  if (fromPeerId === SERVER_PEER_ID) {
    const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS })
    peerConnections.set(fromPeerId, { pc })

    pc.ontrack = (event) => {
      toneEl.srcObject = event.streams[0] ?? new MediaStream([event.track])
      void toneEl.play().catch(() => undefined)
      audioStatusEl.textContent = 'Playing 440 Hz cosine tone from Node server'
    }

    pc.onicecandidate = (event) => {
      if (event.candidate) {
        sendSignal({
          type: 'ice-candidate',
          targetPeerId: fromPeerId,
          candidate: event.candidate.toJSON(),
        })
      }
    }

    await pc.setRemoteDescription(sdp)
    const answer = await pc.createAnswer()
    await pc.setLocalDescription(answer)
    await waitGatheringComplete(pc)
    sendSignal({
      type: 'answer',
      targetPeerId: fromPeerId,
      sdp: pc.localDescription,
    })
    return
  }

  if (!peerConnections.has(fromPeerId)) {
    await connectToPeer(fromPeerId, false)
  }

  const session = peerConnections.get(fromPeerId)
  if (!session) return

  await session.pc.setRemoteDescription(sdp)
  const answer = await session.pc.createAnswer()
  await session.pc.setLocalDescription(answer)
  await waitGatheringComplete(session.pc)
  sendSignal({
    type: 'answer',
    targetPeerId: fromPeerId,
    sdp: session.pc.localDescription,
  })
}

async function onAnswer(fromPeerId, sdp) {
  const session = peerConnections.get(fromPeerId)
  if (!session) return
  await session.pc.setRemoteDescription(sdp)
}

async function onRemoteIce(fromPeerId, candidate) {
  const session = peerConnections.get(fromPeerId)
  if (!session || !candidate?.candidate) return
  await session.pc.addIceCandidate(candidate)
}

function wireChatChannel(peerId, dc) {
  chatChannels.set(peerId, dc)

  dc.onopen = () => {
    appendSystem(`Chat link ready with ${peerId}`)
  }

  dc.onmessage = (event) => {
    try {
      const payload = JSON.parse(String(event.data))
      if (payload.type === 'chat') {
        appendMessage(`${payload.name}: ${payload.text}`)
      }
    } catch {
      appendMessage(`${peerId}: ${event.data}`)
    }
  }
}

function broadcastChat(payload) {
  const body = JSON.stringify(payload)
  for (const dc of chatChannels.values()) {
    if (dc.readyState === 'open') {
      dc.send(body)
    }
  }
}

function sendSignal(message) {
  if (!ws || ws.readyState !== WebSocket.OPEN || !clientId || !room) return
  ws.send(
    JSON.stringify({
      ...message,
      room,
      peerId: clientId,
    }),
  )
}

function waitGatheringComplete(pc) {
  if (pc.iceGatheringState === 'complete') {
    return Promise.resolve()
  }

  return new Promise((resolve) => {
    const check = () => {
      if (pc.iceGatheringState === 'complete') {
        pc.removeEventListener('icegatheringstatechange', check)
        resolve()
      }
    }
    pc.addEventListener('icegatheringstatechange', check)
    check()
  })
}

function appendMessage(text) {
  const item = document.createElement('li')
  item.textContent = text
  messagesEl.appendChild(item)
  messagesEl.scrollTop = messagesEl.scrollHeight
}

function appendSystem(text) {
  const item = document.createElement('li')
  item.className = 'system'
  item.textContent = text
  messagesEl.appendChild(item)
  messagesEl.scrollTop = messagesEl.scrollHeight
}

function setStatus(text) {
  statusEl.textContent = text
}
