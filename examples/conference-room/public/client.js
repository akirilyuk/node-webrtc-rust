const SERVER_PEER_ID = 'conference-server'
const ICE_SERVERS = [{ urls: 'stun:stun.l.google.com:19302' }]

import { attachAudioVisualizer } from '/shared/audio-visualizer.js'

const displayNameInput = document.getElementById('display-name')
const roomInput = document.getElementById('room')
const connectButton = document.getElementById('connect')
const disconnectButton = document.getElementById('disconnect')
const statusEl = document.getElementById('status')
const audioStatusEl = document.getElementById('audio-status')
const mixedAudioEl = document.getElementById('mixed-audio')
const participantsEl = document.getElementById('participants')
const logEl = document.getElementById('log')
const mixingStatusEl = document.getElementById('mixing-status')
const disableMixingButton = document.getElementById('disable-mixing')
const enableMixingButton = document.getElementById('enable-mixing')
const micVizCanvas = document.getElementById('mic-viz')
const incomingVizCanvas = document.getElementById('incoming-viz')
const micVizStatusEl = document.getElementById('mic-viz-status')
const incomingVizStatusEl = document.getElementById('incoming-viz-status')

/** @type {WebSocket | null} */
let ws = null
/** @type {string | null} */
let clientId = null
/** @type {string | null} */
let room = null
/** @type {string} */
let displayName = 'Guest'
/** @type {RTCPeerConnection | null} */
let pc = null
/** @type {MediaStream | null} */
let localStream = null
/** @type {ReturnType<typeof setInterval> | null} */
let participantPollTimer = null
/** @type {{ stop: () => void } | null} */
let micVisualizer = null
/** @type {{ stop: () => void } | null} */
let incomingVisualizer = null

connectButton.addEventListener('click', () => {
  void connect()
})

disconnectButton.addEventListener('click', () => {
  disconnect()
})

disableMixingButton.addEventListener('click', () => {
  void setRoomMixing(false)
})

enableMixingButton.addEventListener('click', () => {
  void setRoomMixing(true)
})

async function connect() {
  if (ws) return

  room = roomInput.value.trim() || 'demo'
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
    setStatus(`Signaling connected — waiting for conference offer…`)
    disconnectButton.disabled = false
    disableMixingButton.disabled = false
    enableMixingButton.disabled = false
    appendLog(`Joined room "${room}" as ${displayName} (${clientId})`)
    startParticipantPolling()
  }

  ws.onmessage = (event) => {
    handleSignal(JSON.parse(event.data))
  }

  ws.onclose = () => {
    appendLog('Signaling disconnected')
    cleanupConnection()
    ws = null
    clientId = null
    connectButton.disabled = false
    disconnectButton.disabled = true
    disableMixingButton.disabled = true
    enableMixingButton.disabled = true
    setStatus('Disconnected')
    mixingStatusEl.textContent = 'Mixing: unknown'
  }

  ws.onerror = () => {
    setStatus('WebSocket error')
  }
}

function disconnect() {
  ws?.close()
  cleanupConnection()
  mixedAudioEl.srcObject = null
  audioStatusEl.textContent = 'Waiting for mixed track…'
}

function cleanupConnection() {
  stopParticipantPolling()
  micVisualizer?.stop()
  micVisualizer = null
  incomingVisualizer?.stop()
  incomingVisualizer = null
  micVizStatusEl.textContent = 'Connect to visualize outgoing audio'
  incomingVizStatusEl.textContent = 'Waiting for mixed track…'
  pc?.close()
  pc = null
  for (const track of localStream?.getTracks() ?? []) {
    track.stop()
  }
  localStream = null
  participantsEl.innerHTML = ''
}

function handleSignal(message) {
  switch (message.type) {
    case 'peer-joined':
      if (message.peerId === SERVER_PEER_ID) {
        appendLog('Conference server present in room')
      } else if (message.peerId !== clientId) {
        appendLog(`${message.peerId} joined signaling room`)
      }
      break
    case 'peer-left':
      appendLog(`${message.peerId} left signaling room`)
      break
    case 'offer':
      if (message.peerId === SERVER_PEER_ID) {
        void onServerOffer(message.sdp)
      }
      break
    case 'ice-candidate':
      if (message.peerId === SERVER_PEER_ID) {
        void onServerIce(message.candidate)
      }
      break
    default:
      break
  }
}

async function onServerOffer(sdp) {
  if (pc) return

  try {
    localStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false })
    micVisualizer?.stop()
    micVisualizer = attachAudioVisualizer({
      canvas: micVizCanvas,
      mediaStream: localStream,
      waveColor: '#34d399',
      barColor: '#10b981',
    })
    micVizStatusEl.textContent = 'Live microphone waveform (outgoing to mixer)'
  } catch (error) {
    setStatus(`Microphone access denied: ${error}`)
    connectButton.disabled = false
    return
  }

  pc = new RTCPeerConnection({ iceServers: ICE_SERVERS })

  for (const track of localStream.getTracks()) {
    pc.addTrack(track, localStream)
  }

  pc.ontrack = (event) => {
    mixedAudioEl.srcObject = event.streams[0] ?? new MediaStream([event.track])
    void mixedAudioEl.play().catch(() => undefined)
    incomingVisualizer?.stop()
    incomingVisualizer = attachAudioVisualizer({
      canvas: incomingVizCanvas,
      audioElement: mixedAudioEl,
      waveColor: '#38bdf8',
      barColor: '#818cf8',
    })
    incomingVisualizer.resume()
    audioStatusEl.textContent = 'Playing personalized mixed audio from conference server'
    incomingVizStatusEl.textContent = 'Live waveform of incoming mixed track'
    appendLog('Receiving mixed audio track')
  }

  pc.onicecandidate = (event) => {
    if (event.candidate) {
      sendSignal({
        type: 'ice-candidate',
        targetPeerId: SERVER_PEER_ID,
        candidate: event.candidate.toJSON(),
      })
    }
  }

  pc.onconnectionstatechange = () => {
    if (!pc) return
    if (pc.connectionState === 'connected') {
      setStatus(`Connected to room "${room}" as ${displayName}`)
    } else if (pc.connectionState === 'failed' || pc.connectionState === 'disconnected') {
      setStatus(`WebRTC ${pc.connectionState}`)
      appendLog(`WebRTC connection ${pc.connectionState}`)
    } else if (pc.connectionState === 'closed') {
      appendLog('WebRTC connection closed (you may have been kicked)')
      disconnect()
    }
  }

  await pc.setRemoteDescription(sdp)
  const answer = await pc.createAnswer()
  await pc.setLocalDescription(answer)
  await waitGatheringComplete(pc)
  sendSignal({
    type: 'answer',
    targetPeerId: SERVER_PEER_ID,
    sdp: pc.localDescription,
  })
  appendLog('Sent answer to conference server')
}

async function onServerIce(candidate) {
  if (!pc || !candidate?.candidate) return
  await pc.addIceCandidate(candidate)
}

function startParticipantPolling() {
  stopParticipantPolling()
  void refreshParticipants()
  participantPollTimer = setInterval(() => {
    void refreshParticipants()
  }, 2000)
}

function stopParticipantPolling() {
  if (participantPollTimer) {
    clearInterval(participantPollTimer)
    participantPollTimer = null
  }
}

async function refreshParticipants() {
  if (!room || !clientId) return

  const response = await fetch(`/api/rooms/${encodeURIComponent(room)}/participants`)
  if (!response.ok) return

  const payload = await response.json()
  const participants = payload.participants ?? []
  const mixingEnabled = payload.mixingEnabled ?? true
  mixingStatusEl.textContent = `Mixing: ${mixingEnabled ? 'enabled' : 'disabled (silence for all)'}`

  const ids = participants.map((entry) => entry.id)
  if (!ids.includes(clientId) && pc) {
    appendLog('No longer in participant list — disconnecting')
    disconnect()
    return
  }

  renderParticipants(participants)
}

function renderParticipants(participants) {
  participantsEl.innerHTML = ''

  for (const participant of participants) {
    const item = document.createElement('li')
    const row = document.createElement('div')
    row.className = 'participant-row'

    const label = document.createElement('div')
    label.className = 'participant-id'
    label.textContent =
      participant.id === clientId ? `${participant.id} (you)` : participant.id

    const state = document.createElement('div')
    state.className = 'participant-state'
    state.textContent = participant.connectionState

    row.appendChild(label)
    row.appendChild(state)

    if (participant.id !== clientId) {
      // Global mute: target is excluded from every listener's mix (moderator action).
      const globalMute = document.createElement('button')
      globalMute.type = 'button'
      globalMute.className = 'secondary'
      globalMute.textContent = 'Global mute'
      globalMute.title = 'Exclude this participant from all listeners mixes'
      globalMute.addEventListener('click', () => {
        void muteParticipant(participant.id, 'global', true)
      })

      const globalUnmute = document.createElement('button')
      globalUnmute.type = 'button'
      globalUnmute.className = 'secondary'
      globalUnmute.textContent = 'Global unmute'
      globalUnmute.addEventListener('click', () => {
        void muteParticipant(participant.id, 'global', false)
      })

      // Listener mute: only this tab stops hearing the target; others unchanged.
      const listenerMute = document.createElement('button')
      listenerMute.type = 'button'
      listenerMute.className = 'secondary'
      listenerMute.textContent = 'Mute for me'
      listenerMute.title = 'Stop hearing this participant in your mix only'
      listenerMute.addEventListener('click', () => {
        void muteParticipant(participant.id, 'listener', true)
      })

      const listenerUnmute = document.createElement('button')
      listenerUnmute.type = 'button'
      listenerUnmute.className = 'secondary'
      listenerUnmute.textContent = 'Unmute for me'
      listenerUnmute.addEventListener('click', () => {
        void muteParticipant(participant.id, 'listener', false)
      })

      const kick = document.createElement('button')
      kick.type = 'button'
      kick.className = 'danger'
      kick.textContent = 'Kick'
      kick.addEventListener('click', () => {
        void kickParticipant(participant.id)
      })

      row.appendChild(globalMute)
      row.appendChild(globalUnmute)
      row.appendChild(listenerMute)
      row.appendChild(listenerUnmute)
      row.appendChild(kick)
    }

    item.appendChild(row)
    participantsEl.appendChild(item)
  }
}

async function muteParticipant(targetId, scope, muted) {
  if (!room || !clientId) return

  const body = { targetId, scope, muted }
  if (scope === 'listener') {
    body.listenerId = clientId
  }

  const response = await fetch(`/api/rooms/${encodeURIComponent(room)}/mute`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })

  if (!response.ok) {
    appendLog(`Mute request failed: ${await response.text()}`)
    return
  }

  if (scope === 'global') {
    appendLog(
      muted
        ? `Global mute applied to ${targetId} (everyone stops hearing them)`
        : `Global unmute applied to ${targetId}`,
    )
  } else {
    appendLog(
      muted
        ? `You muted ${targetId} in your mix only`
        : `You unmuted ${targetId} in your mix`,
    )
  }
}

async function setRoomMixing(enabled) {
  if (!room) return

  const response = await fetch(`/api/rooms/${encodeURIComponent(room)}/mixing`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  })

  if (!response.ok) {
    appendLog(`Mixing toggle failed: ${await response.text()}`)
    return
  }

  appendLog(
    enabled
      ? 'Room mixing enabled — personalized audio resumes'
      : 'Room mixing disabled — silence for all listeners (mute state preserved)',
  )
  void refreshParticipants()
}

async function kickParticipant(participantId) {
  if (!room) return

  const response = await fetch(`/api/rooms/${encodeURIComponent(room)}/kick`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ participantId, reason: 'demo kick' }),
  })

  if (!response.ok) {
    appendLog(`Kick failed: ${await response.text()}`)
    return
  }

  appendLog(`Kicked ${participantId}`)
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

function appendLog(text) {
  const item = document.createElement('li')
  item.className = 'system'
  item.textContent = text
  logEl.prepend(item)
  while (logEl.children.length > 30) {
    logEl.removeChild(logEl.lastChild)
  }
}

function setStatus(text) {
  statusEl.textContent = text
}
