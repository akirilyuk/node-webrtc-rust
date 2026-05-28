/**
 * Browser WebRTC client for the voice-agent browser examples.
 *
 * - Sends microphone audio to the Node VoiceAgent server
 * - Plays agent TTS on #agent-audio
 * - Live mic waveform via shared audio-visualizer
 * - Exchanges JSON on the `voice-control` data channel:
 *     client → { type: 'speak', text }
 *     server → { type: 'speech_event', event, text?, error? }
 */

import { attachAudioVisualizer } from '/shared/audio-visualizer.js'

const SERVER_PEER_ID = 'voice-agent-server'
const VOICE_CONTROL_CHANNEL_LABEL = 'voice-control'
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

const LONG_TTS_TEXT =
  'This is a long agent response for barge-in testing. ' +
  'Keep listening while I talk for several seconds. ' +
  'When you hear this playback, speak into your microphone to interrupt me. ' +
  'You should see a barge_in event in the log and the audio should cut off.'

const roomInput = document.getElementById('room')
const connectButton = document.getElementById('connect')
const disconnectButton = document.getElementById('disconnect')
const statusEl = document.getElementById('status')
const audioEl = document.getElementById('agent-audio')
const audioStatusEl = document.getElementById('audio-status')
const speakForm = document.getElementById('speak-form')
const speakTextInput = document.getElementById('speak-text')
const speakLongButton = document.getElementById('speak-long')
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

speakLongButton.addEventListener('click', () => {
  sendSpeak(LONG_TTS_TEXT)
  appendEvent('client', `Requested long TTS (${LONG_TTS_TEXT.length} chars)`)
})

function setStatus(text) {
  statusEl.textContent = text
}

function appendEvent(eventName, detail) {
  const item = document.createElement('li')
  item.textContent = `${new Date().toISOString().slice(11, 23)} ${eventName}${detail ? `: ${detail}` : ''}`
  if (eventName === 'barge_in') item.classList.add('barge-in')
  if (eventName === 'user_speech_final') item.classList.add('final')
  if (eventName === 'error') item.classList.add('error')
  eventLogEl.prepend(item)
}

function sendSignal(message) {
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(message))
  }
}

/** Signaling server requires room + peerId on offer/answer/ICE forwards. */
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
    micVizStatusEl.textContent = 'Live — waveform shows your mic input sent to the server'
  }
}

function stopMicVisualizer() {
  micVisualizer?.stop()
  micVisualizer = null
  if (micVizStatusEl) {
    micVizStatusEl.textContent = 'Connect and allow microphone to visualize input'
  }
}

function sendSpeak(text) {
  if (!controlChannel || controlChannel.readyState !== 'open') {
    appendEvent('error', 'Control channel not open')
    return
  }
  controlChannel.send(JSON.stringify({ type: 'speak', text }))
  appendEvent('client', `speak → "${text.slice(0, 80)}${text.length > 80 ? '…' : ''}"`)
}

function wireControlChannel(channel) {
  controlChannel = channel
  channel.onopen = () => {
    appendEvent('client', 'voice-control channel open')
    speakTextInput.disabled = false
    speakForm.querySelector('button').disabled = false
    speakLongButton.disabled = false
  }
  channel.onclose = () => {
    appendEvent('client', 'voice-control channel closed')
    controlChannel = null
  }
  channel.onmessage = (event) => {
    try {
      const message = JSON.parse(String(event.data))
      if (message.type !== 'speech_event') return
      const detail = message.text ?? message.error ?? ''
      appendEvent(message.event, detail)
    } catch {
      appendEvent('error', 'Malformed server message')
    }
  }
}

async function connect() {
  if (ws) return

  room = roomInput.value.trim() || 'demo'
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

  try {
    micStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false })
    startMicVisualizer(micStream)
  } catch (error) {
    setStatus('Microphone permission denied')
    connectButton.disabled = false
    return
  }

  const wsUrl = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`
  ws = new WebSocket(wsUrl)

  ws.onopen = () => {
    sendSignal({ type: 'join', room, peerId: clientId })
    setStatus(`Connected to room "${room}" — waiting for server offer…`)
    disconnectButton.disabled = false
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
  audioStatusEl.textContent = 'Connect to hear agent speech'
  speakTextInput.disabled = true
  speakForm.querySelector('button').disabled = true
  speakLongButton.disabled = true
  connectButton.disabled = false
  disconnectButton.disabled = true
  setStatus('Disconnected')
}

function handleSignal(message) {
  switch (message.type) {
    case 'peer-joined':
      if (message.peerId === SERVER_PEER_ID) {
        appendEvent('client', 'Voice agent server joined')
      }
      break
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
    audioStatusEl.textContent = 'Playing agent TTS track'
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
      setStatus(`WebRTC connected — speak into your mic`)
      micVisualizer?.resume()
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

  setStatus('Answer sent — finishing ICE…')
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
