/**
 * Stop any process already listening on a TCP port (dev examples only).
 *
 * Avoids EADDRINUSE when restarting `npm run start` without manually killing the old server.
 * Set `VOICE_SKIP_FREE_PORT=1` to disable. Unix/macOS uses `lsof`; Windows uses `netstat` + `taskkill`.
 */

import { execSync } from 'node:child_process'

function parsePids(output: string): number[] {
  return output
    .split(/\s+/)
    .map((s) => Number.parseInt(s.trim(), 10))
    .filter((pid) => Number.isFinite(pid) && pid > 0 && pid !== process.pid)
}

function pidsOnPortUnix(port: number): number[] {
  try {
    const out = execSync(`lsof -ti :${port} -sTCP:LISTEN`, {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
    if (!out) return []
    return parsePids(out)
  } catch {
    return []
  }
}

function pidsOnPortWindows(port: number): number[] {
  try {
    const out = execSync(`netstat -ano -p tcp | findstr :${port}`, {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    })
    const pids = new Set<number>()
    for (const line of out.split(/\r?\n/)) {
      if (!line.includes('LISTENING')) continue
      const parts = line.trim().split(/\s+/)
      const pid = Number.parseInt(parts[parts.length - 1] ?? '', 10)
      if (Number.isFinite(pid) && pid > 0 && pid !== process.pid) {
        pids.add(pid)
      }
    }
    return [...pids]
  } catch {
    return []
  }
}

function killPid(pid: number): boolean {
  try {
    if (process.platform === 'win32') {
      execSync(`taskkill /PID ${pid} /F`, { stdio: 'ignore' })
    } else {
      process.kill(pid, 'SIGKILL')
    }
    return true
  } catch {
    return false
  }
}

/**
 * Terminate processes listening on `port`. Logs when anything was stopped.
 */
export function freePort(port: number, label = 'server'): void {
  if (process.env.VOICE_SKIP_FREE_PORT === '1') {
    return
  }
  if (!Number.isFinite(port) || port <= 0 || port > 65535) {
    return
  }

  const pids =
    process.platform === 'win32' ? pidsOnPortWindows(port) : pidsOnPortUnix(port)
  if (pids.length === 0) {
    return
  }

  let stopped = 0
  for (const pid of pids) {
    if (killPid(pid)) stopped += 1
  }

  if (stopped > 0) {
    console.log(
      `[free-port] Stopped ${stopped} process(es) on port ${port} before starting ${label}`,
    )
  }
}

const isMain =
  process.argv[1]?.endsWith('free-port.ts') === true ||
  process.argv[1]?.endsWith('free-port.mjs') === true

if (isMain) {
  const fromArg = process.argv[2] ? Number.parseInt(process.argv[2], 10) : NaN
  const port = Number.isFinite(fromArg) ? fromArg : Number(process.env.PORT ?? 0)
  freePort(port, 'CLI')
}
