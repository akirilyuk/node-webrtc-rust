/**
 * Shared WebRTC loopback helpers for voice-agent examples.
 *
 * ## Why a loopback instead of a browser?
 *
 * VoiceAgent needs a real RTCPeerConnection with negotiated Opus audio tracks.
 * These examples spin up **two Node peers** (agent + user) on one machine so you
 * can exercise the full native pipeline without a browser tab or microphone.
 *
 * ## Track directions (easy to confuse)
 *
 * From the **VoiceAgent's perspective** after `attach()`:
 *
 * - `outboundTrack` → `agentOut` on the agent PC — TTS PCM is written here and
 *   heard by the remote "user" peer.
 * - `inboundTrack` → `agentInbound` on the agent PC — PCM from the remote user
 *   (userOut RTP decoded) feeds VAD + STT. **Do not** pass `userInbound` here;
 *   that track carries agent→user audio, not user→agent speech.
 *
 * Run any mock demo:
 *   npm run start:callback --workspace=@node-webrtc-rust/example-voice-agent
 */

import { autoNegotiate, SignalingClient, SignalingServer } from '@node-webrtc-rust/signaling'

import {
  LocalAudioTrack,
  RTCPeerConnection,
  type RemoteAudioTrack,
  type RTCPeerConnection as RTCPeerConnectionType,
} from '@node-webrtc-rust/sdk'

import {
  createKickFrame,
  PCM_FRAME_DURATION_MS,
  PCM_FULL_FRAME_BYTES,
  PCM_KICK_DURATION_MS,
} from '../../shared/pcm-streaming.js'

/** Public STUN — enough for two peers on the same host to find host candidates. */
const STUN = { urls: 'stun:stun.l.google.com:19302' }

/**
 * Convenience wrapper around {@link createBidirectionalLoopback}.
 * Mock demos only need agent-side tracks; live demos call the bidirectional helper directly
 * when they also need `userOut` to synthesize inbound speech.
 */
export async function createLoopbackAudio(): Promise<{
  server: SignalingServer
  agentPc: RTCPeerConnectionType
  userPc: RTCPeerConnectionType
  agentOut: LocalAudioTrack
  agentInbound: RemoteAudioTrack
  userInbound: RemoteAudioTrack
  cleanup: () => Promise<void>
}> {
  const bidirectional = await createBidirectionalLoopback()
  return {
    server: bidirectional.server,
    agentPc: bidirectional.agentPc,
    userPc: bidirectional.userPc,
    agentOut: bidirectional.agentOut,
    agentInbound: bidirectional.agentInbound,
    userInbound: bidirectional.userInbound,
    cleanup: bidirectional.cleanup,
  }
}

/**
 * Builds agent ↔ user audio in **both directions**.
 *
 * Signaling uses `port: 0` so examples never collide with conference demos on 8080.
 * `autoNegotiate` with polite/impolite roles avoids glare when both sides add tracks.
 */
export async function createBidirectionalLoopback(): Promise<{
  server: SignalingServer
  agentPc: RTCPeerConnectionType
  userPc: RTCPeerConnectionType
  agentOut: LocalAudioTrack
  agentInbound: RemoteAudioTrack
  userOut: LocalAudioTrack
  userInbound: RemoteAudioTrack
  cleanup: () => Promise<void>
}> {
  const server = new SignalingServer({ port: 0 })
  await server.listen(0)

  const agentPc = new RTCPeerConnection({ iceServers: [STUN] })
  const userPc = new RTCPeerConnection({ iceServers: [STUN] })

  // Each side advertises one send track; the remote side receives it as ontrack → RemoteAudioTrack.
  const agentOut = new LocalAudioTrack('agent-out', 'voice-demo')
  const userOut = new LocalAudioTrack('user-out', 'voice-demo')
  await agentPc.addTrack(agentOut)
  await userPc.addTrack(userOut)

  const sigAgent = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'voice-demo',
    peerId: 'agent',
  })
  const sigUser = new SignalingClient({
    url: `ws://localhost:${server.port}`,
    room: 'voice-demo',
    peerId: 'user',
  })

  // Agent is impolite (offers first); user is polite — standard two-peer negotiation pattern.
  autoNegotiate({ pc: agentPc, signaling: sigAgent, polite: false })
  autoNegotiate({ pc: userPc, signaling: sigUser, polite: true })

  // Resolve remote tracks before connect completes so attach() never races ontrack.
  const agentInboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for agent ontrack')), 20_000)
    agentPc.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        clearTimeout(timer)
        resolve(event.track as RemoteAudioTrack)
      }
    }
  })

  const userInboundPromise = new Promise<RemoteAudioTrack>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('timed out waiting for user ontrack')), 20_000)
    userPc.ontrack = (event) => {
      if (event.track.kind === 'audio') {
        clearTimeout(timer)
        resolve(event.track as RemoteAudioTrack)
      }
    }
  })

  await sigAgent.connect()
  await sigUser.connect()

  await waitConnected(agentPc)
  await waitConnected(userPc)

  // Kick frames prime RTP so receivers get ontrack + decoders before streaming.
  // See examples/shared/pcm-streaming.ts for the 960 B / 5 ms convention.
  await agentOut.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)
  await userOut.writeSample(createKickFrame(), PCM_KICK_DURATION_MS)

  const [agentInbound, userInbound] = await Promise.all([agentInboundPromise, userInboundPromise])

  return {
    server,
    agentPc,
    userPc,
    agentOut,
    agentInbound,
    userOut,
    userInbound,
    cleanup: async () => {
      agentPc.close()
      userPc.close()
      sigAgent.disconnect()
      sigUser.disconnect()
      await server.close()
    },
  }
}

/**
 * Sends a 440 Hz tone on the **user** leg so the agent's inbound track carries non-silent PCM.
 *
 * Live vendor demos call this before `sendTextToTTS()` so VAD/STT have something to process
 * without requiring a real microphone. Mock STT uses byte thresholds, not frequency detection.
 */
export async function streamUserTone(userOut: LocalAudioTrack, seconds: number): Promise<void> {
  const frameCount = Math.ceil((seconds * 1000) / PCM_FRAME_DURATION_MS)
  for (let i = 0; i < frameCount; i++) {
    await userOut.writeSample(createToneFrame(440), PCM_FRAME_DURATION_MS)
  }
}

/** Generates one 20 ms stereo frame at 48 kHz (3840 bytes) — matches WebRTC PCM conventions. */
function createToneFrame(hz: number): Buffer {
  const buf = Buffer.alloc(PCM_FULL_FRAME_BYTES)
  const samplesPerChannel = PCM_FULL_FRAME_BYTES / 4
  for (let i = 0; i < samplesPerChannel; i++) {
    const t = i / 48_000
    const sample = Math.sin(2 * Math.PI * hz * t) * 16_000
    const clamped = Math.max(-32768, Math.min(32767, Math.floor(sample)))
    buf.writeInt16LE(clamped, i * 4)
    buf.writeInt16LE(clamped, i * 4 + 2)
  }
  return buf
}

async function waitConnected(pc: RTCPeerConnectionType): Promise<void> {
  if (pc.connectionState === 'connected') return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('connection timeout')), 20_000)
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === 'connected') {
        clearTimeout(timer)
        resolve()
      }
    }
  })
}

/**
 * Mock STT/TTS — no API keys, deterministic transcripts and sine-wave TTS in Rust.
 * Use this for CI and for learning the event API before spending cloud credits.
 */
export const mockVoiceConfig = {
  stt: { provider: 'mock' as const, language: 'en' },
  tts: { provider: 'mock' as const, voice: 'demo' },
}
