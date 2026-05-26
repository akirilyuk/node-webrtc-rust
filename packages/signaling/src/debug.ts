const DEBUG_ENV_VALUES = new Set(['1', 'true', 'yes'])

let configOverride: boolean | undefined

function parseEnv(value: string): boolean {
  return DEBUG_ENV_VALUES.has(value.trim().toLowerCase())
}

/** Returns whether `[webrtc-debug]` logging is enabled (env var or config override). */
export function isDebugEnabled(): boolean {
  if (configOverride !== undefined) {
    return configOverride
  }
  const env = process.env.WEBRTC_DEBUG
  if (env === undefined) {
    return false
  }
  return parseEnv(env)
}

/** Overrides the debug flag for signaling helpers. */
export function setDebugEnabled(enabled: boolean): void {
  configOverride = enabled
}

/** Logs a function call when debug mode is enabled. */
export function debugFn(module: string, fnName: string, args = ''): void {
  if (!isDebugEnabled()) {
    return
  }
  const suffix = args.length > 0 ? `(${args})` : '()'
  console.error(`[webrtc-debug] ${module}::${fnName}${suffix}`)
}

/** Logs an event emission when debug mode is enabled. */
export function debugEvent(module: string, event: string, detail = ''): void {
  if (!isDebugEnabled()) {
    return
  }
  const suffix = detail.length > 0 ? ` ${detail}` : ''
  console.error(`[webrtc-debug] ${module} event ${event}${suffix}`)
}
