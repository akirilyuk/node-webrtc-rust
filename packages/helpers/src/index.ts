export {
  PCM_SAMPLE_RATE,
  PCM_CHANNELS,
  PCM_FRAME_DURATION_MS,
  PCM_FULL_FRAME_BYTES,
  PCM_KICK_FRAME_BYTES,
  PCM_KICK_DURATION_MS,
  createKickFrame,
} from './pcm.js'

/**
 * Capability marker for capacity-safe teardown / quarantine APIs.
 * Runners must refuse startup when this export is missing or below 1.
 */
export const HELPERS_CAPACITY_SAFE_TEARDOWN = 1 as const

export {
  VOICE_AGENT_SERVER_PEER_ID,
  SERVER_PEER_ID,
  VoiceAgentSessionHost,
  type VoiceAgentSessionHostOptions,
  type PeerCloseOutcome,
  type TeardownComponentStatus,
} from './voice-agent-session-host.js'

export {
  type VoiceSessionContext,
  type VoiceSessionHandler,
  type DataChannelKind,
} from './voice-session-handler.js'

export {
  flushVoiceControlChannel,
  VOICE_CONTROL_FLUSH_POLL_MS,
  VOICE_CONTROL_FLUSH_TIMEOUT_MS,
} from './control-channel-flush.js'

export {
  SessionPod,
  type SessionPodChangeEvent,
  type SessionPodCloseOutcome,
  type SessionPodOptions,
  type SessionPodSessionInfo,
} from './session-pod.js'
export {
  SessionPodCapacityFullError,
  SessionPodRecycleRequiredError,
} from './session-pod-errors.js'

export {
  VoiceSessionBudget,
  VoiceSessionBudgetFullError,
  getProcessVoiceSessionBudget,
  resetProcessVoiceSessionBudget,
  resolveMaxVoiceSessionsFromEnv,
  type VoiceSessionBudgetOptions,
  type VoiceSessionBudgetSnapshot,
  type VoiceSessionLease,
} from './voice-session-budget.js'

export {
  startMultiClientVoiceServer,
  formatBudget,
  type MultiClientVoiceServerHandle,
  type MultiClientVoiceServerOptions,
} from './multi-client-voice-server.js'

export {
  SessionRecorder,
  pcmFromWriteSampleTeeArgs,
  resolveSessionRecorderOptionsFromEnv,
  SESSION_RECORDER_DEFAULT_MAX_SEC,
  SESSION_RECORDER_DEFAULT_OPUS_BITRATE_BPS,
  type SessionRecorderFinalizeResult,
  type SessionRecorderFormat,
  type SessionRecorderOptions,
} from './session-recorder.js'
