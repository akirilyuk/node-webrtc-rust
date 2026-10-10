#!/usr/bin/env node
// Aggregate the speech engine probes into one JSON file and markdown tables.
//
//   node scripts/perf/voice-cost-report.mjs --out <report.json> --md <report.md> <raw logs...>
//
// Input: `perf_probe {json}` lines (the shape collect-perf.mjs parses) from
// `tts_voice_cost_probe` and `stt_stream_capacity_probe`, as written by
// scripts/perf/run-voice-cost.sh. The same point measured in several runs (several lines with the
// same name and params, in one log or many) is reduced to the median of each metric; a boolean
// metric is true when more than half of the runs were true.
//
// TTS table, one row per voice (and thread count):
//   RTF                 tts_long_rtf_p50 at 1 session (wall seconds per audio second)
//   cost vs amy-medium  tts_cpu_s_per_audio_s at 1 session over the same figure of
//                       vits-piper-en_US-amy-medium (same thread count); "-" without that baseline
//   first audio         tts_short_ttfa_ms p50/p95 at 1 session, p95 at 2 and 4 sessions
// STT table, one per recognizer: lag/final p95 per stream count, and the capacity, the largest
// tested stream count that stayed real-time.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { pathToFileURL } from 'node:url'

export const BASELINE_VOICE = 'vits-piper-en_US-amy-medium'
const TTS_NAME = 'tts_voice_cost_probe'
const STT_NAME = 'stt_stream_capacity_probe'

export function median(values) {
  const sorted = [...values].sort((a, b) => a - b)
  const mid = Math.floor(sorted.length / 2)
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2
}

/** Every `perf_probe {json}` line of `text`, parsed. */
export function parseProbeLines(text) {
  const probes = []
  for (const line of text.split('\n')) {
    const at = line.indexOf('perf_probe ')
    if (at === -1) continue
    probes.push(JSON.parse(line.slice(at + 'perf_probe '.length)))
  }
  return probes
}

/** Group probes by name + params; reduce each metric to its median over the runs. */
export function aggregateRuns(probes) {
  const groups = new Map()
  for (const probe of probes) {
    const key = `${probe.name}|${JSON.stringify(probe.params, Object.keys(probe.params).sort())}`
    if (!groups.has(key)) groups.set(key, { name: probe.name, params: probe.params, runs: [] })
    groups.get(key).runs.push(probe.metrics)
  }
  return [...groups.values()].map(({ name, params, runs }) => {
    const metrics = {}
    for (const metric of new Set(runs.flatMap((run) => Object.keys(run)))) {
      const values = runs.map((run) => run[metric]).filter((value) => value !== undefined)
      metrics[metric] =
        typeof values[0] === 'boolean'
          ? values.filter(Boolean).length > values.length / 2
          : median(values)
    }
    return { name, params, metrics, runs: runs.length }
  })
}

/** Largest stream count whose point stayed real-time, or 0 when none did. */
export function sttCapacity(points) {
  return points
    .filter((point) => point.realtime_ok)
    .reduce((best, point) => Math.max(best, point.streams), 0)
}

export function buildReport(aggregated) {
  const tts = aggregated.filter((point) => point.name === TTS_NAME)
  const stt = aggregated.filter((point) => point.name === STT_NAME)

  const voices = new Map()
  for (const point of tts) {
    const { model, tts_num_threads: threads } = point.params
    const key = `${model}|${threads}`
    if (!voices.has(key)) voices.set(key, { model, threads, bySessions: new Map(), runs: 0 })
    const voice = voices.get(key)
    voice.bySessions.set(point.params.sessions, point.metrics)
    voice.runs = Math.max(voice.runs, point.runs)
  }
  const cpuAtOne = (model, threads) =>
    voices.get(`${model}|${threads}`)?.bySessions.get(1)?.tts_cpu_s_per_audio_s
  const ttsRows = [...voices.values()]
    .map(({ model, threads, bySessions, runs }) => {
      const one = bySessions.get(1) ?? {}
      const baseline = cpuAtOne(BASELINE_VOICE, threads)
      const ttfa = {}
      for (const [sessions, metrics] of bySessions) {
        ttfa[sessions] = {
          p50: metrics.tts_short_ttfa_ms_p50,
          p95: metrics.tts_short_ttfa_ms_p95,
        }
      }
      return {
        model,
        threads,
        runs,
        rtf: one.tts_long_rtf_p50 ?? null,
        cost_vs_amy_medium:
          baseline && one.tts_cpu_s_per_audio_s !== undefined
            ? one.tts_cpu_s_per_audio_s / baseline
            : null,
        cpu_s_per_audio_s: one.tts_cpu_s_per_audio_s ?? null,
        ttfa_ms: ttfa,
        rss_mb_peak: Math.max(...[...bySessions.values()].map((m) => m.rss_mb_peak ?? 0)),
      }
    })
    .sort((a, b) => a.model.localeCompare(b.model) || a.threads - b.threads)

  const recognizers = new Map()
  for (const point of stt) {
    const { model, stt_num_threads: threads } = point.params
    const key = `${model}|${threads}`
    if (!recognizers.has(key)) recognizers.set(key, { model, threads, points: [] })
    recognizers.get(key).points.push({
      streams: point.params.streams,
      lag_ms_p95: point.metrics.stt_lag_ms_p95,
      final_ms_p95: point.metrics.stt_final_ms_p95,
      realtime_ok: point.metrics.stt_realtime_ok,
      cpu_s_per_audio_s: point.metrics.cpu_s_per_audio_s,
      rss_mb_peak: point.metrics.rss_mb_peak,
      runs: point.runs,
    })
  }
  const sttGroups = [...recognizers.values()].map((group) => {
    group.points.sort((a, b) => a.streams - b.streams)
    return { ...group, capacity_streams: sttCapacity(group.points) }
  })

  return { tts: ttsRows, stt: sttGroups }
}

const fixed = (value, digits) => (typeof value === 'number' ? value.toFixed(digits) : '-')

export function renderMarkdown(report) {
  const lines = []
  if (report.tts.length > 0) {
    const showThreads = new Set(report.tts.map((row) => row.threads)).size > 1
    lines.push(
      '### TTS cost per voice',
      '',
      '| Voice | RTF | Cost vs amy-medium | First audio, 1 session p50 / p95 (ms) | First audio p95, 2 sessions (ms) | First audio p95, 4 sessions (ms) | CPU s per audio s | RSS (MB) |',
      '| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |',
    )
    for (const row of report.tts) {
      const one = row.ttfa_ms[1]
      lines.push(
        `| ${row.model}${showThreads ? ` (${row.threads} threads)` : ''} | ${fixed(row.rtf, 2)} | ${
          row.cost_vs_amy_medium === null ? '-' : `${fixed(row.cost_vs_amy_medium, 2)}x`
        } | ${one ? `${fixed(one.p50, 0)} / ${fixed(one.p95, 0)}` : '-'} | ${fixed(
          row.ttfa_ms[2]?.p95,
          0,
        )} | ${fixed(row.ttfa_ms[4]?.p95, 0)} | ${fixed(row.cpu_s_per_audio_s, 3)} | ${fixed(
          row.rss_mb_peak,
          0,
        )} |`,
      )
    }
    lines.push('')
  }
  for (const group of report.stt) {
    lines.push(
      `### STT streams (${group.model}, ${group.threads} threads)`,
      '',
      '| Streams | Lag p95 (ms) | Final p95 (ms) | Real-time ok |',
      '| ---: | ---: | ---: | :---: |',
    )
    for (const point of group.points) {
      lines.push(
        `| ${point.streams} | ${fixed(point.lag_ms_p95, 0)} | ${fixed(point.final_ms_p95, 0)} | ${
          point.realtime_ok ? 'yes' : 'no'
        } |`,
      )
    }
    lines.push(
      '',
      `Capacity: ${group.capacity_streams} stream${group.capacity_streams === 1 ? '' : 's'} real-time.`,
      '',
    )
  }
  return lines.join('\n')
}

function main(argv) {
  let out
  let md
  const logs = []
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--out') out = argv[++i]
    else if (argv[i] === '--md') md = argv[++i]
    else logs.push(argv[i])
  }
  if (!out || !md || logs.length === 0) {
    console.error('usage: voice-cost-report.mjs --out <report.json> --md <report.md> <raw logs...>')
    return 2
  }
  const probes = logs.flatMap((log) => parseProbeLines(readFileSync(log, 'utf8')))
  if (probes.length === 0) {
    console.error('voice-cost-report: no perf_probe lines found')
    return 1
  }
  const report = buildReport(aggregateRuns(probes))
  const markdown = renderMarkdown(report)
  for (const path of [out, md]) mkdirSync(dirname(path), { recursive: true })
  writeFileSync(out, `${JSON.stringify(report, null, 2)}\n`)
  writeFileSync(md, `${markdown}\n`)
  console.log(markdown)
  console.log(`voice-cost-report: wrote ${out} and ${md} (${probes.length} probe lines)`)
  return 0
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exit(main(process.argv.slice(2)))
}
