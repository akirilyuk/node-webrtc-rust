// JS main-thread cost probe for any Node.js process (preload with NODE_OPTIONS=--import).
//
//   PERF_PROBE=1 PERF_PROBE_MATCH=<argv substring> NODE_OPTIONS="--import /abs/path/js-thread-probe.mjs" <command>
//
// Active only when PERF_PROBE=1 and (if set) one argv entry contains PERF_PROBE_MATCH.
// Every PERF_PROBE_INTERVAL_MS (default 5000) it prints one line to stderr with window and
// cumulative values:
//   js_thread_cpu_ms_per_s  CPU time of the JS main thread (process.threadCpuUsage), per wall second
//   cpu_ms_per_s            process CPU time over all threads (process.cpuUsage), per wall second
//   elu                     event-loop utilization (performance.eventLoopUtilization)
//   delay p50/p99/max       event-loop delay (monitorEventLoopDelay, 10 ms resolution)
// Use js_thread_cpu_ms_per_s for absolute JS load. Do not attach a CPU profiler while measuring:
// the inspector profiler inflates event-loop utilization several times.
import { monitorEventLoopDelay, performance } from 'node:perf_hooks'
import process from 'node:process'
import { setInterval } from 'node:timers'

const match = process.env.PERF_PROBE_MATCH
const active =
  process.env.PERF_PROBE === '1' && (!match || process.argv.some((a) => a.includes(match)))

if (active) {
  const tag = `[perf-probe pid=${process.pid}]`
  const intervalMs = Number(process.env.PERF_PROBE_INTERVAL_MS ?? 5000)
  const win = monitorEventLoopDelay({ resolution: 10 })
  const cum = monitorEventLoopDelay({ resolution: 10 })
  win.enable()
  cum.enable()
  const t0 = performance.now()
  const elu0 = performance.eventLoopUtilization()
  const cpu0 = process.cpuUsage()
  const th0 = process.threadCpuUsage ? process.threadCpuUsage() : null
  let last = { t: t0, elu: elu0, cpu: cpu0, th: th0 }
  const ms = (ns) => (ns / 1e6).toFixed(1)
  const perS = (usedUs, wallMs) => (usedUs / 1000 / (wallMs / 1000)).toFixed(0)
  const sumUs = (u) => u.user + u.system

  const timer = setInterval(() => {
    const now = performance.now()
    const elu = performance.eventLoopUtilization(last.elu)
    const cpu = process.cpuUsage(last.cpu)
    const th = th0 ? process.threadCpuUsage(last.th) : null
    const cumCpu = process.cpuUsage(cpu0)
    const cumTh = th0 ? process.threadCpuUsage(th0) : null
    process.stderr.write(
      `${tag} t=${((now - t0) / 1000).toFixed(0)}s ` +
        `js_thread_cpu_ms_per_s=${th ? perS(sumUs(th), now - last.t) : 'n/a'} ` +
        `cpu_ms_per_s=${perS(sumUs(cpu), now - last.t)} elu=${elu.utilization.toFixed(3)} ` +
        `delay_p50=${ms(win.percentile(50))} p99=${ms(win.percentile(99))} max=${ms(win.max)} | ` +
        `cum_js_thread_cpu_ms_per_s=${cumTh ? perS(sumUs(cumTh), now - t0) : 'n/a'} ` +
        `cum_cpu_ms_per_s=${perS(sumUs(cumCpu), now - t0)} ` +
        `cum_elu=${performance.eventLoopUtilization(elu0).utilization.toFixed(3)} ` +
        `cum_delay_p99=${ms(cum.percentile(99))} cum_delay_max=${ms(cum.max)}\n`,
    )
    win.reset()
    last = {
      t: now,
      elu: performance.eventLoopUtilization(),
      cpu: process.cpuUsage(),
      th: th0 ? process.threadCpuUsage() : null,
    }
  }, intervalMs)
  timer.unref()
}
