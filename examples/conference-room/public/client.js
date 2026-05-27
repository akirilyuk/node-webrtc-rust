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
/** @type {Map<string, string>} */
const peerDisplayNames = new Map()

/** @param {string} peerId @param {string} [knownName] */
function participantLabel(peerId, knownName) {
  if (peerId === clientId) return displayName
  if (knownName) return knownName
  return peerDisplayNames.get(peerId) ?? 'Unknown'
}

async function registerDisplayName() {
  if (!room || !clientId) return
  await fetch(`/api/rooms/${encodeURIComponent(room)}/display-name`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ peerId: clientId, displayName }),
  })
}

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
    void registerDisplayName().then(() => refreshParticipants())
    appendLog(`Joined room "${room}" as ${displayName}`)
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
        void refreshParticipants().then(() => {
          appendLog(`${participantLabel(message.peerId)} joined the room`)
        })
      }
      break
    case 'peer-left':
      peerDisplayNames.delete(message.peerId)
      appendLog(`${participantLabel(message.peerId)} left the room`)
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

  const attachMixedAudio = (track, stream) => {
    if (mixedAudioEl.srcObject) return
    const playStream = stream ?? new MediaStream([track])
    mixedAudioEl.srcObject = playStream
    void mixedAudioEl.play().catch(() => undefined)
    incomingVisualizer?.stop()
    incomingVisualizer = attachAudioVisualizer({
      canvas: incomingVizCanvas,
      mediaStream: playStream,
      waveColor: '#38bdf8',
      barColor: '#818cf8',
    })
    incomingVisualizer.resume()
    audioStatusEl.textContent = 'Playing personalized mixed audio from conference server'
    incomingVizStatusEl.textContent = 'Live waveform of incoming mixed track'
    appendLog('Receiving mixed audio track')
  }

  pc.ontrack = (event) => {
    attachMixedAudio(event.track, event.streams[0])
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
      if (!mixedAudioEl.srcObject) {
        for (const receiver of pc.getReceivers()) {
          if (receiver.track?.kind === 'audio') {
            attachMixedAudio(receiver.track, null)
            break
          }
        }
      }
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

  for (const entry of participants) {
    if (entry.displayName) {
      peerDisplayNames.set(entry.id, entry.displayName)
    }
  }

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

    const name = participantLabel(participant.id, participant.displayName)
    const label = document.createElement('div')
    label.className = 'participant-id'
    label.textContent = participant.id === clientId ? `${name} (you)` : name

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

  const targetName = participantLabel(targetId)
  if (scope === 'global') {
    appendLog(
      muted
        ? `Global mute applied to ${targetName} (everyone stops hearing them)`
        : `Global unmute applied to ${targetName}`,
    )
  } else {
    appendLog(
      muted
        ? `You muted ${targetName} in your mix only`
        : `You unmuted ${targetName} in your mix`,
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

  appendLog(`Kicked ${participantLabel(participantId)}`)
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
