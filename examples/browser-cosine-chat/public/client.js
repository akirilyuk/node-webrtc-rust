import { attachAudioVisualizer } from '/shared/audio-visualizer.js'

const SERVER_PEER_ID = 'cosine-server'
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]
const DEBUG = new URLSearchParams(location.search).has('debug')

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
const incomingVizCanvas = document.getElementById('incoming-viz')
const vizStatusEl = document.getElementById('viz-status')
const debugPanelEl = document.getElementById('debug-panel')
const debugLogEl = document.getElementById('debug-log')

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
/** @type {Map<string, string>} */
const peerNames = new Map()
/** @type {Map<string, RTCDataChannel>} */
const chatChannels = new Map()
/** @type {Map<string, RTCIceCandidateInit[]>} */
const pendingIceByPeer = new Map()
/** @type {{ stop: () => void } | null} */
let incomingVisualizer = null

if (DEBUG && debugPanelEl && debugLogEl) {
  debugPanelEl.hidden = false
  debugLog('Debug mode on — reload with ?debug if you change this URL mid-session')
}

function debugLog(...args) {
  if (!DEBUG) return
  const line = args.map((arg) => (typeof arg === 'string' ? arg : JSON.stringify(arg))).join(' ')
  console.log('[browser-cosine-chat]', ...args)
  if (debugLogEl) {
    debugLogEl.textContent += `${new Date().toISOString().slice(11, 23)} ${line}\n`
    debugLogEl.scrollTop = debugLogEl.scrollHeight
  }
}

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
  debugLog(`connect room=${room} clientId=${clientId}`)
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
  incomingVisualizer?.stop()
  incomingVisualizer = null
  vizStatusEl.textContent = 'Waiting for server track…'
  for (const { pc } of peerConnections.values()) {
    pc.close()
  }
  peerConnections.clear()
  chatChannels.clear()
  peerNames.clear()
  pendingIceByPeer.clear()
}

function handleSignal(message) {
  debugLog(`signal type=${message.type} peer=${message.peerId ?? message.targetPeerId ?? '-'}`)
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

function peerLabel(peerId) {
  return peerNames.get(peerId) ?? peerId
}

/**
 * Log W3C peer connection state transitions when ?debug is present.
 * Node SDK exposes the same callbacks on RTCPeerConnection (parity v0.2).
 */
function attachPeerStateDebug(pc, label) {
  if (!DEBUG) return
  pc.onicegatheringstatechange = () => {
    debugLog(`${label} iceGatheringState=${pc.iceGatheringState}`)
  }
  pc.onsignalingstatechange = () => {
    debugLog(`${label} signalingState=${pc.signalingState}`)
  }
  pc.oniceconnectionstatechange = () => {
    debugLog(`${label} iceConnectionState=${pc.iceConnectionState}`)
  }
}

/** Poll inbound RTP counters when ?debug is in the URL. */
function startInboundRtpDebug(pc, label) {
  if (!DEBUG) return
  debugLog(`${label}: RTP stats polling started`)
  let lastBytes = 0
  let polls = 0
  setInterval(async () => {
    polls++
    try {
      const stats = await pc.getStats()
      let found = false
      stats.forEach((report) => {
        if (report.type === 'inbound-rtp' && report.kind === 'audio') {
          found = true
          const delta = report.bytesReceived - lastBytes
          lastBytes = report.bytesReceived
          debugLog(
            `${label} rtp packets=${report.packetsReceived} bytes=${report.bytesReceived} (+${delta}/2s) state=${pc.connectionState}`,
          )
        }
      })
      if (!found) {
        debugLog(`${label} rtp poll #${polls}: no inbound-rtp audio yet, pc=${pc.connectionState}`)
      }
    } catch (error) {
      debugLog(`${label} rtp getStats failed: ${error}`)
    }
  }, 2000)
}

function onPeerJoined(peerId) {
  if (!clientId || peerId === clientId || peerConnections.has(peerId)) return

  if (peerId === SERVER_PEER_ID) {
    appendSystem('Server joined — waiting for audio offer')
    return
  }

  appendSystem('Someone joined the room')

  // Lexicographic tie-break: higher id creates the chat offer.
  if (clientId > peerId) {
    void connectToPeer(peerId, true)
  }
}

function onPeerLeft(peerId) {
  appendSystem(`${peerLabel(peerId)} left the room`)
  peerNames.delete(peerId)
  pendingIceByPeer.delete(peerId)
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
  attachPeerStateDebug(pc, `peer-${peerId}`)
  startInboundRtpDebug(pc, `peer-${peerId}`)

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
    try {
      debugLog('server offer received')
      const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS })
      peerConnections.set(fromPeerId, { pc })
      attachPeerStateDebug(pc, 'server-audio')
      startInboundRtpDebug(pc, 'server-audio')

      const attachServerAudio = (track, stream, source) => {
        if (toneEl.srcObject) {
          debugLog(`attach skipped (already have srcObject) source=${source}`)
          return
        }
        debugLog(`attach audio track id=${track.id} muted=${track.muted} source=${source}`)
        track.onunmute = () => {
          debugLog(`track unmuted id=${track.id}`)
          void toneEl.play().catch(() => undefined)
        }
        track.onmute = () => debugLog(`track muted id=${track.id}`)
        track.onended = () => debugLog(`track ended id=${track.id}`)
        const playStream = stream ?? new MediaStream([track])
        toneEl.srcObject = playStream
        void toneEl.play().catch(() => undefined)
        incomingVisualizer?.stop()
        incomingVisualizer = attachAudioVisualizer({
          canvas: incomingVizCanvas,
          mediaStream: playStream,
          waveColor: '#fbbf24',
          barColor: '#f59e0b',
        })
        incomingVisualizer.resume()
        audioStatusEl.textContent = 'Playing 440 Hz cosine tone from Node server'
        vizStatusEl.textContent = 'Live waveform of incoming server audio track'
      }

      pc.ontrack = (event) => {
        debugLog(`ontrack kind=${event.track.kind} id=${event.track.id}`)
        attachServerAudio(event.track, event.streams[0], 'ontrack')
      }

      pc.onconnectionstatechange = () => {
        debugLog(`server pc connectionState=${pc.connectionState}`)
        if (pc.connectionState !== 'connected' || toneEl.srcObject) return
        for (const receiver of pc.getReceivers()) {
          if (receiver.track?.kind === 'audio') {
            attachServerAudio(receiver.track, null, 'getReceivers')
            break
          }
        }
      }

      pc.oniceconnectionstatechange = () => {
        debugLog(`server pc iceConnectionState=${pc.iceConnectionState}`)
      }

      pc.onicecandidate = (event) => {
        if (event.candidate) {
          debugLog(`server pc local ice → cosine-server`)
          sendSignal({
            type: 'ice-candidate',
            targetPeerId: fromPeerId,
            candidate: event.candidate.toJSON(),
          })
        }
      }

      await pc.setRemoteDescription(sdp)
      await flushPendingIce(fromPeerId)
      debugLog(`server pc remoteDescription set, receivers=${pc.getReceivers().length}`)

      for (const receiver of pc.getReceivers()) {
        if (receiver.track?.kind === 'audio') {
          attachServerAudio(receiver.track, null, 'post-setRemoteDescription')
          break
        }
      }

      const answer = await pc.createAnswer()
      await pc.setLocalDescription(answer)
      await waitGatheringComplete(pc)
      debugLog('server answer sent → cosine-server')
      sendSignal({
        type: 'answer',
        targetPeerId: fromPeerId,
        sdp: pc.localDescription,
      })
    } catch (error) {
      debugLog(`server offer handling FAILED: ${error}`)
      console.error(error)
    }
    return
  }

  if (!peerConnections.has(fromPeerId)) {
    await connectToPeer(fromPeerId, false)
  }

  const session = peerConnections.get(fromPeerId)
  if (!session) return

  await session.pc.setRemoteDescription(sdp)
  await flushPendingIce(fromPeerId)
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
  await flushPendingIce(fromPeerId)
}

async function onRemoteIce(fromPeerId, candidate) {
  const session = peerConnections.get(fromPeerId)
  if (!session || !candidate?.candidate) return

  if (!session.pc.remoteDescription) {
    if (!pendingIceByPeer.has(fromPeerId)) {
      pendingIceByPeer.set(fromPeerId, [])
    }
    pendingIceByPeer.get(fromPeerId).push(candidate)
    return
  }

  try {
    await session.pc.addIceCandidate(candidate)
  } catch {
    // Ignore stale candidates after reconnect.
  }
}

async function flushPendingIce(peerId) {
  const session = peerConnections.get(peerId)
  const pending = pendingIceByPeer.get(peerId)
  if (!session?.pc.remoteDescription || !pending?.length) return

  for (const candidate of pending) {
    try {
      await session.pc.addIceCandidate(candidate)
    } catch {
      // Ignore stale candidates.
    }
  }
  pendingIceByPeer.delete(peerId)
}

function wireChatChannel(peerId, dc) {
  chatChannels.set(peerId, dc)

  // Node SDK parity: onbufferedamountlow when bufferedAmount <= bufferedAmountLowThreshold.
  if (DEBUG) {
    dc.bufferedAmountLowThreshold = 256 * 1024
    dc.onbufferedamountlow = () => {
      debugLog(`chat dc bufferedamountlow peer=${peerId} amount=${dc.bufferedAmount}`)
    }
  }

  dc.onopen = () => {
    dc.send(JSON.stringify({ type: 'hello', name: displayName }))
    appendSystem('Chat link established')
  }

  dc.onmessage = (event) => {
    try {
      const payload = JSON.parse(String(event.data))
      if (payload.type === 'hello' && payload.name) {
        peerNames.set(peerId, payload.name)
        appendSystem(`${payload.name} joined the chat`)
        return
      }
      if (payload.type === 'chat') {
        appendMessage(`${payload.name}: ${payload.text}`)
        return
      }
    } catch {
      appendMessage(`${peerLabel(peerId)}: ${event.data}`)
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
