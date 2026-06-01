/**
 * Shared env + failure diagnostics for Sherpa roundtrip / voice E2E scripts.
 *
 * Rust (`crates/speech`, Sherpa STT) reads **`VOICE_DEBUG=1`** (also `true` / `yes`) and
 * prints `[voice-debug]` lines to stderr. There is no separate `DEBUG_LOGS` env in this repo.
 */

import { basename } from 'node:path'
import { fileURLToPath } from 'node:url'

import type { UtteranceEventStats } from './roundtrip-counting.js'
import { enableRoundtripSpeechEventLog } from './roundtrip-speech-events.js'

const VOICE_DEBUG_TRUTHY = new Set(['1', 'true', 'yes'])

/** Set when `installRoundtripWallClockTimeout` runs from an entry script's `main()`. */
let roundtripEntryScript = basename(fileURLToPath(import.meta.url), '.ts')

function isVoiceDebugEnabled(): boolean {
  const v = process.env.VOICE_DEBUG
  return v !== undefined && VOICE_DEBUG_TRUTHY.has(v)
}

const ROUNDTRIP_ENTRY_RE = /[/\\](roundtrip(?:-[\w]+)?\.ts):\d+/

/** Caller entry script (basename without `.ts`), from stack — same idea as caller `__filename`. */
function roundtripScriptFromStack(): string {
  const stack = new Error().stack ?? ''
  for (const line of stack.split('\n')) {
    if (line.includes('roundtrip-failure-debug')) continue
    if (line.includes('roundtrip-counting.')) continue
    const m = line.match(ROUNDTRIP_ENTRY_RE)
    if (m) {
      return basename(m[1], '.ts')
    }
  }
  return roundtripEntryScript
}

/**
 * Enable Rust voice pipeline logs for the current process **before** `VoiceAgent.start()`.
 * Called from `installRoundtripWallClockTimeout` for every `start:roundtrip*` script.
 *
 * Default: **on** for roundtrips. Opt out: `SHERPA_ROUNDTRIP_DEBUG=0` or `VOICE_DEBUG=0`.
 */
export function enableSherpaRoundtripRustDebug(): void {
  enableRoundtripSpeechEventLog()
  const scriptName = roundtripScriptFromStack()
  const roundtripDebug = process.env.SHERPA_ROUNDTRIP_DEBUG
  const voiceDebug = process.env.VOICE_DEBUG

  const disable =
    roundtripDebug === '0' ||
    roundtripDebug === 'false' ||
    voiceDebug === '0' ||
    voiceDebug === 'false'

  if (disable) {
    return
  }

  if (isVoiceDebugEnabled()) {
    console.error(`[${scriptName}] VOICE_DEBUG=${voiceDebug} — Rust [voice-debug] on stderr`)
    return
  }

  process.env.VOICE_DEBUG = '1'
  if (process.env.SHERPA_COUNTING_VERBOSE !== '0') {
    process.env.SHERPA_COUNTING_VERBOSE = '1'
  }
  console.error(
    `[${scriptName}] VOICE_DEBUG=1 — Rust agent/STT pipeline logs on stderr ([voice-debug]); ` +
      'SHERPA_COUNTING_VERBOSE=1 for TS speech events; disable with SHERPA_ROUNDTRIP_DEBUG=0',
  )
}

export type RoundtripLegDebug = {
  label: string
  recognized?: string
  phrase?: string
  stats?: UtteranceEventStats
}

export type SherpaRoundtripFailureContext = {
  reason: string
  failures?: string[]
  legs?: RoundtripLegDebug[]
  error?: unknown
}

function formatStatsLine(label: string, stats: UtteranceEventStats): string {
  const finalsPreview =
    stats.finals.length === 0
      ? '(none)'
      : stats.finals.map((t, i) => `#${i + 1} "${t.trim()}"`).join('; ')
  const timing =
    stats.speakingEndAtMs != null && stats.speechFinalAtMs != null
      ? ` end→final ${stats.speechFinalAtMs - stats.speakingEndAtMs}ms`
      : ''
  return (
    `[${label}] finals=${stats.finals.length} partials=${stats.partialCount} ` +
    `speaking_start=${stats.speakingStartCount} speaking_end=${stats.speakingEndCount} ` +
    `barge_in=${stats.bargeInCount}${timing}\n` +
    `    finals: ${finalsPreview}`
  )
}

/** Print structured failure context without exiting (tests may call this directly). */
export function reportSherpaRoundtripFailure(ctx: SherpaRoundtripFailureContext): void {
  const script = roundtripScriptFromStack()
  console.error(`\n=== [${script}] roundtrip failure: ${ctx.reason} ===`)

  if (ctx.failures?.length) {
    console.error('Assertions:')
    for (const msg of ctx.failures) {
      console.error(`  - ${msg}`)
    }
  }

  if (ctx.legs?.length) {
    console.error('Legs:')
    for (const leg of ctx.legs) {
      if (leg.phrase) {
        console.error(`  ${leg.label} phrase: "${leg.phrase}"`)
      }
      if (leg.recognized !== undefined) {
        console.error(`  ${leg.label} recognized: "${leg.recognized}"`)
      }
      if (leg.stats) {
        console.error(`  ${formatStatsLine(leg.label, leg.stats)}`)
      }
    }
  }

  if (ctx.error !== undefined) {
    console.error('Error:', ctx.error)
  }

  const countingVerbose = process.env.SHERPA_COUNTING_VERBOSE
  console.error('\n--- Pipeline logs (same run + re-run) ---')
  if (isVoiceDebugEnabled()) {
    console.error(
      'VOICE_DEBUG is on — scroll stderr above for [voice-debug] (Rust VAD/STT/gate-hold).',
    )
  } else {
    console.error(
      'VOICE_DEBUG was off — roundtrips normally set it at startup; re-run without SHERPA_ROUNDTRIP_DEBUG=0.',
    )
  }
  if (countingVerbose !== '1') {
    console.error('For TS speech events add SHERPA_COUNTING_VERBOSE=1 (or set at start of script).')
  }
  console.error('Re-run example:')
  console.error(
    `  SHERPA_COUNTING_VERBOSE=1 npm run start:<script> --workspace=@node-webrtc-rust/example-voice-agent-local-sherpa`,
  )
  console.error(
    'Prior write-ups: development/node-webrtc-rust/features/ (see dev-logs-retrospective rule).',
  )
}

/** Print diagnostics and exit with code 1. */
export function exitSherpaRoundtripFailure(ctx: SherpaRoundtripFailureContext): never {
  reportSherpaRoundtripFailure(ctx)
  process.exit(1)
}

/** Entry script from `tsx src/roundtrip-….ts` (reliable under tsx; stack regex is fallback). */
function roundtripScriptFromArgv(): string | undefined {
  const entry = process.argv[1]
  if (entry == null || !entry.includes('roundtrip')) {
    return undefined
  }
  return basename(entry).replace(/\.(ts|js|mts|cts)$/, '')
}

/** Remember entry script for async wall-clock timeout (stack has no entry frame in timer). */
export function rememberRoundtripEntryScript(): void {
  roundtripEntryScript = roundtripScriptFromArgv() ?? roundtripScriptFromStack()
}

/** Entry script name (from caller stack or `rememberRoundtripEntryScript`). */
export function currentRoundtripScript(): string {
  return roundtripScriptFromStack()
}
