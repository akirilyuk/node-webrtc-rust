/**
 * Browser client for voice-agent-local-sherpa-multi-client.
 *
 * Each tab uses a stable client id (`client-tab1` …) from ?slot= so server logs
 * are easy to correlate. Checks GET /api/capacity before connecting.
 */

import { attachAudioVisualizer } from '/shared/audio-visualizer.js'

const SERVER_PEER_ID = 'voice-agent-server'
const VOICE_CONTROL_CHANNEL_LABEL = 'voice-control'
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

const roomInput = document.getElementById('room')
const connectButton = document.getElementById('connect')
const disconnectButton = document.getElementById('disconnect')
const statusEl = document.getElementById('status')
const capacityStatusEl = document.getElementById('capacity-status')
const audioEl = document.getElementById('agent-audio')
const audioStatusEl = document.getElementById('audio-status')
const speakForm = document.getElementById('speak-form')
const speakTextInput = document.getElementById('speak-text')
const eventLogEl = document.getElementById('event-log')
const micVizCanvas = document.getElementById('mic-viz')
const micVizStatusEl = document.getElementById('mic-viz-status')

/** @type {WebSocket | null} */
let ws = null
/** @type {string | null} */
let clientId = null
/** @type {string | null} */
let room = null
/** @type {RTCPeerConnection | null} */
let serverPc = null
/** @type {RTCDataChannel | null} */
let controlChannel = null
/** @type {MediaStream | null} */
let micStream = null
/** @type {RTCIceCandidateInit[]} */
let pendingIce = []
/** @type {ReturnType<typeof attachAudioVisualizer> | null} */
let micVisualizer = null

const slotParam = new URLSearchParams(location.search).get('slot')?.trim()
const tabSlot =
  slotParam && /^[1-9]$/.test(slotParam) ? slotParam : String(Math.floor(Math.random() * 900) + 100)

connectButton.addEventListener('click', () => {
  void connect()
})

disconnectButton.addEventListener('click', () => {
  disconnect()
})

speakForm.addEventListener('submit', (event) => {
  event.preventDefault()
  const text = speakTextInput.value.trim()
  if (!text) return
  sendSpeak(text)
  speakTextInput.value = ''
})

function setStatus(text) {
  statusEl.textContent = text
}

function appendEvent(eventName, detail) {
  const item = document.createElement('li')
  item.textContent = `${new Date().toISOString().slice(11, 23)} ${eventName}${detail ? `: ${detail}` : ''}`
  if (eventName === 'user_speech_final') item.classList.add('final')
  if (eventName === 'error') item.classList.add('error')
  eventLogEl.prepend(item)
}

async function refreshCapacity() {
  try {
    const response = await fetch('/api/capacity')
    const payload = await response.json()
    const voice = payload.voice ?? payload
    const max = voice.max ?? 0
    const line =
      max === 0
        ? `Capacity: unlimited (active=${voice.active})`
        : `Capacity: ${voice.active}/${max} available=${voice.available} rejected=${voice.rejectedTotal}`
    capacityStatusEl.textContent = line
    return voice
  } catch {
    capacityStatusEl.textContent = 'Capacity: unknown'
    return null
  }
}

void refreshCapacity()
setInterval(() => void refreshCapacity(), 3000)

function sendSignal(message) {
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(message))
  }
}

function sendToServer(payload) {
  sendSignal({
    room,
    peerId: clientId,
    ...payload,
  })
}

function startMicVisualizer(stream) {
  if (!micVizCanvas) return
  micVisualizer?.stop()
  micVisualizer = attachAudioVisualizer({
    canvas: micVizCanvas,
    mediaStream: stream,
    waveColor: '#34d399',
    barColor: '#10b981',
  })
  if (micVizStatusEl) {
    micVizStatusEl.textContent = 'Live mic → server STT'
  }
}

function stopMicVisualizer() {
  micVisualizer?.stop()
  micVisualizer = null
  if (micVizStatusEl) {
    micVizStatusEl.textContent = 'Connect and allow microphone'
  }
}

function sendSpeak(text) {
  if (!controlChannel || controlChannel.readyState !== 'open') {
    appendEvent('error', 'Control channel not open')
    return
  }
  controlChannel.send(JSON.stringify({ type: 'speak', text }))
  appendEvent('client', `speak → "${text.slice(0, 80)}"`)
}

function wireControlChannel(channel) {
  controlChannel = channel
  channel.onopen = () => {
    appendEvent('client', 'voice-control open')
    speakTextInput.disabled = false
    speakForm.querySelector('button').disabled = false
  }
  channel.onclose = () => {
    controlChannel = null
  }
  channel.onmessage = (event) => {
    try {
      const message = JSON.parse(String(event.data))
      if (message.type !== 'speech_event') return
      appendEvent(message.event, message.text ?? message.error ?? '')
    } catch {
      appendEvent('error', 'Malformed server message')
    }
  }
}

async function connect() {
  if (ws) return

  room = roomInput.value.trim() || 'sherpa-multi'
  clientId = `client-tab${tabSlot}`

  const capacity = await refreshCapacity()
  if (capacity && capacity.max > 0 && capacity.available <= 0) {
    setStatus(
      'Server at capacity — wait for a tab to disconnect or raise VOICE_MAX_CONCURRENT_SESSIONS',
    )
    appendEvent('error', 'capacity full')
    return
  }

  setStatus(`Joining ${room} as ${clientId}…`)
  connectButton.disabled = true

  try {
    micStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false })
    startMicVisualizer(micStream)
  } catch {
    setStatus('Microphone permission denied')
    connectButton.disabled = false
    return
  }

  const wsUrl = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`
  ws = new WebSocket(wsUrl)

  ws.onopen = () => {
    sendSignal({ type: 'join', room, peerId: clientId })
    setStatus(`Signaling joined — waiting for offer from server…`)
    disconnectButton.disabled = false
    appendEvent('client', `joined as ${clientId}`)
  }

  ws.onmessage = (event) => {
    handleSignal(JSON.parse(event.data))
  }

  ws.onclose = () => {
    disconnect()
  }

  ws.onerror = () => {
    setStatus('WebSocket error')
  }

  setTimeout(() => {
    if (!serverPc && ws) {
      setStatus('No offer yet — if budget is full, server rejected this tab (see stderr)')
      appendEvent('error', 'no offer within 15s')
      connectButton.disabled = false
    }
  }, 15_000)
}

function disconnect() {
  stopMicVisualizer()
  controlChannel?.close()
  controlChannel = null
  serverPc?.close()
  serverPc = null
  pendingIce = []
  micStream?.getTracks().forEach((track) => track.stop())
  micStream = null
  ws?.close()
  ws = null
  clientId = null
  audioEl.srcObject = null
  speakTextInput.disabled = true
  speakForm.querySelector('button').disabled = true
  connectButton.disabled = false
  disconnectButton.disabled = true
  setStatus('Disconnected')
  void refreshCapacity()
}

function handleSignal(message) {
  switch (message.type) {
    case 'offer':
      if (message.peerId === SERVER_PEER_ID) {
        void onServerOffer(message.sdp)
      }
      break
    case 'ice-candidate':
      if (message.peerId === SERVER_PEER_ID) {
        void onRemoteIce(message.candidate)
      }
      break
    default:
      break
  }
}

async function onServerOffer(sdp) {
  if (serverPc) return

  serverPc = new RTCPeerConnection({ iceServers: ICE_SERVERS })

  serverPc.ontrack = (event) => {
    if (event.track.kind !== 'audio') return
    const stream = event.streams[0] ?? new MediaStream([event.track])
    audioEl.srcObject = stream
    void audioEl.play().catch(() => undefined)
    audioStatusEl.textContent = 'Playing agent TTS'
  }

  serverPc.ondatachannel = (event) => {
    if (event.channel.label === VOICE_CONTROL_CHANNEL_LABEL) {
      wireControlChannel(event.channel)
    }
  }

  serverPc.onicecandidate = (event) => {
    if (event.candidate) {
      sendToServer({
        type: 'ice-candidate',
        targetPeerId: SERVER_PEER_ID,
        candidate: event.candidate.toJSON(),
      })
    }
  }

  serverPc.onconnectionstatechange = () => {
    if (serverPc?.connectionState === 'connected') {
      setStatus(`WebRTC connected (${clientId})`)
      void refreshCapacity()
    }
  }

  await serverPc.setRemoteDescription(sdp)
  await flushPendingIce()

  if (micStream) {
    for (const track of micStream.getAudioTracks()) {
      serverPc.addTrack(track, micStream)
    }
  }

  const answer = await serverPc.createAnswer()
  await serverPc.setLocalDescription(answer)
  await waitGatheringComplete(serverPc)

  sendToServer({
    type: 'answer',
    targetPeerId: SERVER_PEER_ID,
    sdp: serverPc.localDescription,
  })
}

async function onRemoteIce(candidate) {
  if (!serverPc || !candidate?.candidate) return
  if (!serverPc.remoteDescription) {
    pendingIce.push(candidate)
    return
  }
  await serverPc.addIceCandidate(candidate)
}

async function flushPendingIce() {
  if (!serverPc) return
  for (const candidate of pendingIce) {
    await serverPc.addIceCandidate(candidate)
  }
  pendingIce = []
}

function waitGatheringComplete(pc) {
  if (pc.iceGatheringState === 'complete') return Promise.resolve()
  return new Promise((resolve) => {
    const check = () => {
      if (pc.iceGatheringState === 'complete') {
        pc.removeEventListener('icegatheringstatechange', check)
        resolve()
      }
    }
    pc.addEventListener('icegatheringstatechange', check)
  })
}
