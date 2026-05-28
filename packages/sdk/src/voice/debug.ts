const DEBUG_ENV_VALUES = new Set(['1', 'true', 'yes'])

/** Whether `[voice-debug]` logging is enabled (`VOICE_DEBUG=1`). */
export function isVoiceDebugEnabled(): boolean {
  const env = process.env.VOICE_DEBUG
  if (env === undefined) {
    return false
  }
  return DEBUG_ENV_VALUES.has(env.trim().toLowerCase())
}

/** Logs to stderr when voice debug mode is enabled. */
export function voiceDebugLog(module: string, message: string): void {
  if (!isVoiceDebugEnabled()) {
    return
  }
  console.error(`[voice-debug] ${module}: ${message}`)
}
