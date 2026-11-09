import { localWorkerLimit, type LocalLimitOptions } from './workers'

// Worker cap for a package vitest config. Vitest 2 builds its pool with
// minWorkers defaulting to cpus-1; a maxWorkers below that makes tinypool throw
// "minThreads and maxThreads must not conflict", so the minimum is lowered too.
// Returns {} in CI. See ./workers.ts.
export async function workerCap(opts: LocalLimitOptions = {}): Promise<{
  maxWorkers?: number
  minWorkers?: number
}> {
  const { maxWorkers } = await localWorkerLimit(opts)
  return maxWorkers === undefined ? {} : { maxWorkers, minWorkers: 1 }
}
