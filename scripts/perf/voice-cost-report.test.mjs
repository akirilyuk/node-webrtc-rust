// Run: node --test scripts/perf/voice-cost-report.test.mjs
import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  aggregateRuns,
  buildReport,
  parseProbeLines,
  renderMarkdown,
  sttCapacity,
} from './voice-cost-report.mjs'

function tts(model, sessions, metrics) {
  return `perf_probe ${JSON.stringify({
    name: 'tts_voice_cost_probe',
    params: { model, sessions, tts_num_threads: 1 },
    metrics,
  })}`
}

function stt(streams, lag, ok) {
  return `perf_probe ${JSON.stringify({
    name: 'stt_stream_capacity_probe',
    params: { model: 'zipformer-en', streams, stt_num_threads: 1 },
    metrics: {
      stt_lag_ms_p95: lag,
      stt_final_ms_p95: lag + 100,
      stt_realtime_ok: ok,
      cpu_s_per_audio_s: 0.1,
      rss_mb_peak: 300,
    },
  })}`
}

const TTS_LOG = [
  'noise line without a probe',
  tts('vits-piper-en_US-amy-medium', 1, {
    tts_short_ttfa_ms_p50: 100,
    tts_short_ttfa_ms_p95: 120,
    tts_long_rtf_p50: 0.2,
    tts_cpu_s_per_audio_s: 0.2,
    rss_mb_peak: 400,
  }),
  tts('vits-piper-en_US-amy-medium', 2, {
    tts_short_ttfa_ms_p50: 150,
    tts_short_ttfa_ms_p95: 190,
    tts_cpu_s_per_audio_s: 0.2,
    rss_mb_peak: 410,
  }),
  tts('vits-piper-en_US-lessac-high', 1, {
    tts_short_ttfa_ms_p50: 300,
    tts_short_ttfa_ms_p95: 360,
    tts_long_rtf_p50: 0.6,
    tts_cpu_s_per_audio_s: 0.6,
    rss_mb_peak: 500,
  }),
].join('\n')

test('cost vs amy-medium is the ratio of CPU seconds per audio second at one session', () => {
  const report = buildReport(aggregateRuns(parseProbeLines(TTS_LOG)))
  const byModel = Object.fromEntries(report.tts.map((row) => [row.model, row]))
  assert.equal(byModel['vits-piper-en_US-amy-medium'].cost_vs_amy_medium, 1)
  assert.ok(Math.abs(byModel['vits-piper-en_US-lessac-high'].cost_vs_amy_medium - 3) < 1e-9)
  assert.equal(byModel['vits-piper-en_US-amy-medium'].ttfa_ms[2].p95, 190)
  assert.equal(byModel['vits-piper-en_US-lessac-high'].ttfa_ms[2], undefined)
})

test('cost vs amy-medium is null without the baseline voice', () => {
  const log = tts('vits-piper-en_US-lessac-high', 1, {
    tts_short_ttfa_ms_p50: 1,
    tts_short_ttfa_ms_p95: 2,
    tts_long_rtf_p50: 0.6,
    tts_cpu_s_per_audio_s: 0.6,
    rss_mb_peak: 500,
  })
  const report = buildReport(aggregateRuns(parseProbeLines(log)))
  assert.equal(report.tts[0].cost_vs_amy_medium, null)
})

test('runs of the same point reduce to the median per metric', () => {
  const runs = [0.2, 0.9, 0.3].map((cpu) =>
    tts('vits-piper-en_US-amy-medium', 1, {
      tts_short_ttfa_ms_p50: 100,
      tts_short_ttfa_ms_p95: 120,
      tts_long_rtf_p50: cpu,
      tts_cpu_s_per_audio_s: cpu,
      rss_mb_peak: 400,
    }),
  )
  const aggregated = aggregateRuns(parseProbeLines(runs.join('\n')))
  assert.equal(aggregated.length, 1)
  assert.equal(aggregated[0].runs, 3)
  assert.equal(aggregated[0].metrics.tts_long_rtf_p50, 0.3)
})

test('STT capacity is the largest stream count that stayed real-time', () => {
  const log = [stt(1, 20, true), stt(4, 80, true), stt(8, 900, false)].join('\n')
  const report = buildReport(aggregateRuns(parseProbeLines(log)))
  assert.equal(report.stt[0].capacity_streams, 4)
  assert.equal(sttCapacity([]), 0)
  assert.equal(sttCapacity([{ streams: 1, realtime_ok: false }]), 0)
})

test('real-time flag of an STT point is the majority over runs', () => {
  const log = [stt(8, 100, true), stt(8, 100, true), stt(8, 900, false)].join('\n')
  const aggregated = aggregateRuns(parseProbeLines(log))
  assert.equal(aggregated[0].metrics.stt_realtime_ok, true)
  assert.equal(aggregated[0].metrics.stt_lag_ms_p95, 100)
})

test('markdown has a TTS row per voice and the STT capacity line', () => {
  const log = `${TTS_LOG}\n${[stt(1, 20, true), stt(4, 80, true), stt(8, 900, false)].join('\n')}`
  const markdown = renderMarkdown(buildReport(aggregateRuns(parseProbeLines(log))))
  assert.match(markdown, /\| vits-piper-en_US-lessac-high \| 0\.60 \| 3\.00x \| 300 \/ 360 \|/)
  assert.match(markdown, /\| 8 \| 900 \| 1000 \| no \|/)
  assert.match(markdown, /Capacity: 4 streams real-time\./)
})
