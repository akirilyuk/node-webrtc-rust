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
const broadcastForm = document.getElementById('broadcast-form')
const broadcastTextInput = document.getElementById('broadcast-text')
const broadcastStatusEl = document.getElementById('broadcast-status')
const eventLogEl = document.getElementById('event-log')
const micVizCanvas = document.getElementById('mic-viz')
const micVizStatusEl = document.getElementById('mic-viz-status')
const pauseMicBackgroundCheckbox = document.getElementById('pause-mic-background')
const micBackgroundStatusEl = document.getElementById('mic-background-status')

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
/** When true, mute mic tracks while `document.hidden` (default false = allow background capture). */
let pauseMicWhenBackground = false

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

broadcastForm.addEventListener('submit', (event) => {
  event.preventDefault()
  void sendBroadcastSpeak()
})

function setStatus(text) {
  statusEl.textContent = text
}

function appendEvent(eventName, detail, serverTs) {
  const item = document.createElement('li')
  const localTs = new Date().toISOString().slice(11, 23)
  const serverPart =
    serverTs && typeof serverTs === 'string' ? ` srv=${serverTs.slice(11, 23)}` : ''
  item.textContent = `${localTs}${serverPart} ${eventName}${detail ? `: ${detail}` : ''}`
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

const pauseMicBackgroundParam = new URLSearchParams(location.search).get('pauseMicBackground')
if (pauseMicBackgroundCheckbox && pauseMicBackgroundParam === '1') {
  pauseMicBackgroundCheckbox.checked = true
  pauseMicWhenBackground = true
  updateMicBackgroundPolicy()
}

if (pauseMicBackgroundCheckbox) {
  pauseMicBackgroundCheckbox.addEventListener('change', () => {
    pauseMicWhenBackground = pauseMicBackgroundCheckbox.checked
    updateMicBackgroundPolicy()
    appendEvent(
      'client',
      pauseMicWhenBackground ? 'pause mic in background: on' : 'pause mic in background: off',
    )
  })
}

document.addEventListener('visibilitychange', () => {
  applyMicCaptureForVisibility()
})

function updateMicBackgroundPolicy() {
  if (!micBackgroundStatusEl) return
  if (pauseMicWhenBackground) {
    micBackgroundStatusEl.textContent = document.hidden
      ? 'Enabled — mic paused while this tab is in the background.'
      : 'Enabled — mic will pause when you switch to another tab.'
  } else {
    micBackgroundStatusEl.textContent =
      'Default: off — mic keeps sending while this tab is open (even in the background).'
  }
  applyMicCaptureForVisibility()
}

/**
 * Mute/unmute local mic tracks sent over WebRTC. Does not disconnect.
 * When pauseMicWhenBackground is false, tracks stay enabled whenever connected.
 */
function applyMicCaptureForVisibility() {
  if (!micStream) return

  const tracks = micStream.getAudioTracks()
  const shouldCapture = !pauseMicWhenBackground || !document.hidden

  for (const track of tracks) {
    track.enabled = shouldCapture
  }

  if (micVizStatusEl && serverPc) {
    if (!shouldCapture) {
      micVizStatusEl.textContent = 'Mic paused (tab in background)'
    } else {
      micVizStatusEl.textContent = 'Live mic → server STT'
    }
  }
}

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
    applyMicCaptureForVisibility()
    if (micStream?.getAudioTracks().some((t) => t.enabled)) {
      micVizStatusEl.textContent = 'Live mic → server STT'
    }
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

async function sendBroadcastSpeak() {
  const text = broadcastTextInput.value.trim()
  if (!text) {
    broadcastStatusEl.textContent = 'Enter text to broadcast'
    return
  }

  broadcastStatusEl.textContent = 'Broadcasting…'
  broadcastForm.querySelector('button').disabled = true

  try {
    const response = await fetch('/api/broadcast-speak', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text }),
    })
    const payload = await response.json()
    if (!response.ok || !payload.ok) {
      throw new Error(payload.error ?? `HTTP ${response.status}`)
    }
    const peers = Array.isArray(payload.peerIds) ? payload.peerIds.join(', ') : 'none'
    broadcastStatusEl.textContent = `Sent to ${payload.count} client(s): ${peers || '(none connected)'}`
    appendEvent('broadcast', `"${text.slice(0, 80)}" → ${payload.count} tab(s)`)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    broadcastStatusEl.textContent = `Broadcast failed: ${message}`
    appendEvent('error', `broadcast: ${message}`)
  } finally {
    broadcastForm.querySelector('button').disabled = false
  }
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
      const detail = message.text ?? message.error ?? ''
      appendEvent(message.event, detail, message.ts)
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
    applyMicCaptureForVisibility()
    updateMicBackgroundPolicy()
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
    try {
      handleSignal(JSON.parse(event.data))
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      appendEvent('error', `signaling parse/handle: ${message}`)
      setStatus(`Signaling error: ${message}`)
    }
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
    case 'peer-joined':
      if (message.peerId === SERVER_PEER_ID) {
        appendEvent('client', 'voice agent in room — waiting for WebRTC offer…')
        setStatus('Server peer present — waiting for offer…')
      }
      break
    case 'offer':
      if (message.peerId === SERVER_PEER_ID) {
        appendEvent('client', 'received WebRTC offer')
        void onServerOffer(message.sdp).catch((error) => {
          const text = error instanceof Error ? error.message : String(error)
          appendEvent('error', `offer handling: ${text}`)
          setStatus(`WebRTC offer failed: ${text}`)
          connectButton.disabled = false
        })
      } else {
        appendEvent('error', `unexpected offer from ${message.peerId ?? 'unknown'}`)
      }
      break
    case 'ice-candidate':
      if (message.peerId === SERVER_PEER_ID) {
        void onRemoteIce(message.candidate).catch((error) => {
          const text = error instanceof Error ? error.message : String(error)
          appendEvent('error', `ICE: ${text}`)
        })
      }
      break
    default:
      break
  }
}

async function onServerOffer(sdp) {
  if (serverPc) {
    const state = serverPc.connectionState
    if (state === 'failed' || state === 'closed') {
      serverPc.close()
      serverPc = null
      pendingIce = []
      appendEvent('client', `replacing ${state} peer connection (server reconnect)`)
    } else {
      appendEvent('error', 'ignored duplicate offer (disconnect first)')
      return
    }
  }

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

  serverPc.oniceconnectionstatechange = () => {
    const ice = serverPc?.iceConnectionState
    if (ice && ice !== 'new') {
      appendEvent('client', `iceConnectionState=${ice}`)
    }
    if (ice === 'failed') {
      setStatus(
        'ICE failed — restart server with WEBRTC_NAT_1TO1_IPS=127.0.0.1 and use http://127.0.0.1:3004',
      )
    }
  }

  serverPc.onconnectionstatechange = () => {
    const state = serverPc?.connectionState
    if (state && state !== 'new') {
      appendEvent('client', `connectionState=${state}`)
    }
    if (serverPc?.connectionState === 'connected') {
      setStatus(`WebRTC connected (${clientId})`)
      void refreshCapacity()
    } else if (state === 'failed') {
      setStatus('WebRTC failed — server may send a new offer; or Disconnect and Connect')
      serverPc?.close()
      serverPc = null
      pendingIce = []
      connectButton.disabled = false
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

  // Send answer immediately (trickle ICE). Blocking on full gathering delays the answer
  // past ICE failure when STUN is slow — see dev log 2026-06-02-multi-client-ice-first-connect.
  sendToServer({
    type: 'answer',
    targetPeerId: SERVER_PEER_ID,
    sdp: serverPc.localDescription,
  })
  appendEvent('client', 'answer sent (trickle ICE)')
  setStatus('Answer sent — finishing ICE…')

  void waitGatheringComplete(serverPc, 5000).catch(() => undefined)
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

function waitGatheringComplete(pc, timeoutMs = 5000) {
  if (pc.iceGatheringState === 'complete') return Promise.resolve()
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pc.removeEventListener('icegatheringstatechange', check)
      reject(new Error(`ICE gathering timeout (${pc.iceGatheringState})`))
    }, timeoutMs)
    const check = () => {
      if (pc.iceGatheringState === 'complete') {
        clearTimeout(timer)
        pc.removeEventListener('icegatheringstatechange', check)
        resolve()
      }
    }
    pc.addEventListener('icegatheringstatechange', check)
  })
}
