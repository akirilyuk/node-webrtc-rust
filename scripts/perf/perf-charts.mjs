#!/usr/bin/env node
// Render the charts in docs/performance.md from a JSON spec, with no dependencies.
//
//   node scripts/perf/perf-charts.mjs docs/perf-data/2026-10-16/charts.json docs/perf-data/charts
//
// Each chart in the spec becomes <out>/<id>.svg. The SVGs carry their own light and dark
// colors (prefers-color-scheme), so they read on GitHub in both themes.
//
// Chart kinds:
//   hbar  rows of grouped horizontal bars: { rows: [label], series: [{ name, slot, values, labels? }] }
//         scale "log" for values that span orders of magnitude; `labels` overrides the value text.
//   line  one line per series over a numeric x: { series: [{ name, slot, points: [[x, y]] }] },
//         optional `threshold: { value, label }` drawn as a dashed rule.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

const W = 800
const PAD = { left: 24, right: 24 }
const FONT = "system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif"

// Categorical slots 1-3 validate all-pairs in both modes; the dark column is the same hues stepped
// for the dark surface.
const SLOTS = [
  { light: '#2a78d6', dark: '#3987e5' },
  { light: '#eb6834', dark: '#d95926' },
  { light: '#1baf7a', dark: '#199e70' },
]

const esc = (s) =>
  String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')

function style(slotsUsed) {
  const light = slotsUsed
    .map((s) => `.s${s}{fill:${SLOTS[s].light}}.l${s}{stroke:${SLOTS[s].light}}`)
    .join('')
  const dark = slotsUsed
    .map((s) => `.s${s}{fill:${SLOTS[s].dark}}.l${s}{stroke:${SLOTS[s].dark}}`)
    .join('')
  return `<style>
text{font-family:${FONT}}
.bg{fill:#fcfcfb}.t1{fill:#0b0b0b}.t2{fill:#52514e}.t3{fill:#6f6e69}
.grid{stroke:#e6e5e0;stroke-width:1}.axis{stroke:#b9b8b2;stroke-width:1}
.thr{stroke:#52514e;stroke-width:1.5;stroke-dasharray:6 4}.ring{stroke:#fcfcfb}
${light}
@media (prefers-color-scheme: dark){
.bg{fill:#1a1a19}.t1{fill:#ffffff}.t2{fill:#c3c2b7}.t3{fill:#a3a29a}
.grid{stroke:#33332f}.axis{stroke:#57564f}.thr{stroke:#c3c2b7}.ring{stroke:#1a1a19}
${dark}
}
</style>`
}

/** Wrap `text` into lines of at most `max` characters. */
function wrap(text, max) {
  const lines = []
  let cur = ''
  for (const word of String(text).split(/ +/)) {
    if (cur && (cur + ' ' + word).length > max) {
      lines.push(cur)
      cur = word
    } else {
      cur = cur ? cur + ' ' + word : word
    }
  }
  if (cur) lines.push(cur)
  return lines
}

function header(chart) {
  const out = []
  let y = 34
  out.push(
    `<text class="t1" x="${PAD.left}" y="${y}" font-size="20" font-weight="600">${esc(chart.title)}</text>`,
  )
  if (chart.subtitle) {
    for (const line of wrap(chart.subtitle, 100)) {
      y += 20
      out.push(`<text class="t2" x="${PAD.left}" y="${y}" font-size="14">${esc(line)}</text>`)
    }
  }
  return { svg: out.join(''), bottom: y }
}

function legend(series, y) {
  if (series.length < 2) return { svg: '', bottom: y }
  let x = PAD.left
  const out = []
  for (const s of series) {
    out.push(`<rect class="s${s.slot}" x="${x}" y="${y + 4}" width="12" height="12" rx="2"/>`)
    out.push(`<text class="t2" x="${x + 18}" y="${y + 15}" font-size="13">${esc(s.name)}</text>`)
    x += 18 + s.name.length * 7.4 + 22
  }
  return { svg: out.join(''), bottom: y + 20 }
}

function footer(chart, y) {
  const out = []
  for (const line of wrap(chart.note ?? '', 118)) {
    y += 16
    out.push(`<text class="t3" x="${PAD.left}" y="${y}" font-size="12">${esc(line)}</text>`)
  }
  return { svg: out.join(''), bottom: y }
}

function niceMax(v) {
  const p = 10 ** Math.floor(Math.log10(v))
  for (const m of [1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]) if (m * p >= v) return m * p
  return 10 * p
}

function ticksLinear(max, n = 5) {
  const step = max / n
  return Array.from({ length: n + 1 }, (_, i) => i * step)
}

function fmtTick(v, unit) {
  const s = Number.isInteger(v) ? String(v) : String(Number(v.toFixed(2)))
  return unit ? `${s}${unit}` : s
}

/** Bar path with a 4 px rounded data end, square at the baseline. */
function barPath(x0, x1, y, h) {
  const r = Math.min(4, (x1 - x0) / 2, h / 2)
  return `M${x0},${y}H${x1 - r}Q${x1},${y} ${x1},${y + r}V${y + h - r}Q${x1},${y + h} ${x1 - r},${y + h}H${x0}Z`
}

function renderHbar(chart) {
  const head = header(chart)
  const leg = legend(chart.series, head.bottom + 14)
  const labelW = chart.labelWidth ?? 230
  const valueW = 90
  const x0 = PAD.left + labelW
  const x1 = W - PAD.right - valueW
  const barH = 18
  const gap = 2
  const groupGap = 22
  const log = chart.scale === 'log'
  const all = chart.series.flatMap((s) => s.values).filter((v) => v != null)
  let scale, ticks
  if (log) {
    const lo = 10 ** Math.floor(Math.log10(Math.min(...all)))
    const hi = 10 ** Math.ceil(Math.log10(Math.max(...all)))
    scale = (v) =>
      x0 +
      ((Math.log10(Math.max(v, lo)) - Math.log10(lo)) / (Math.log10(hi) - Math.log10(lo))) *
        (x1 - x0)
    ticks = []
    for (let t = lo; t <= hi; t *= 10) ticks.push(t)
  } else {
    const max = chart.max ?? niceMax(Math.max(...all))
    scale = (v) => x0 + (v / max) * (x1 - x0)
    ticks = ticksLinear(max, chart.ticks ?? 5)
  }
  const top = leg.bottom + 18
  const groupH = chart.series.length * (barH + gap) - gap
  const plotH = chart.rows.length * groupH + (chart.rows.length - 1) * groupGap
  const out = []
  for (const t of ticks) {
    const x = scale(t)
    out.push(`<line class="grid" x1="${x}" y1="${top - 6}" x2="${x}" y2="${top + plotH + 6}"/>`)
    const label = chart.tickLabels?.[String(t)] ?? fmtTick(t, chart.tickUnit)
    out.push(
      `<text class="t3" x="${x}" y="${top + plotH + 24}" font-size="12" text-anchor="middle">${esc(label)}</text>`,
    )
  }
  chart.rows.forEach((row, ri) => {
    const gy = top + ri * (groupH + groupGap)
    const lines = wrap(row, Math.floor(labelW / 7.2))
    const ly = gy + groupH / 2 - ((lines.length - 1) * 16) / 2 + 5
    lines.forEach((line, li) => {
      out.push(
        `<text class="t1" x="${PAD.left}" y="${ly + li * 16}" font-size="14">${esc(line)}</text>`,
      )
    })
    chart.series.forEach((s, si) => {
      const v = s.values[ri]
      const y = gy + si * (barH + gap)
      if (v == null) return
      const xe = Math.max(scale(v), x0 + 3)
      out.push(
        `<path class="s${s.slot}" d="${barPath(x0, xe, y, barH)}"><title>${esc(`${s.name} · ${row}: ${s.labels?.[ri] ?? v}`)}</title></path>`,
      )
      const text = s.labels?.[ri] ?? fmtTick(v, chart.valueUnit)
      out.push(
        `<text class="t1" x="${xe + 6}" y="${y + 13.5}" font-size="13" font-weight="600">${esc(text)}</text>`,
      )
    })
  })
  out.push(`<line class="axis" x1="${x0}" y1="${top - 6}" x2="${x0}" y2="${top + plotH + 6}"/>`)
  let bottom = top + plotH + 24
  if (chart.axisLabel) {
    bottom += 18
    out.push(
      `<text class="t2" x="${(x0 + x1) / 2}" y="${bottom}" font-size="13" text-anchor="middle">${esc(chart.axisLabel)}</text>`,
    )
  }
  const foot = footer(chart, bottom + 10)
  return finish(chart, [head.svg, leg.svg, out.join(''), foot.svg], foot.bottom + 18)
}

function renderLine(chart) {
  const head = header(chart)
  const leg = legend(chart.series, head.bottom + 14)
  const x0 = PAD.left + 56
  const x1 = W - PAD.right - 70
  const top = leg.bottom + 24
  const plotH = chart.height ?? 300
  const yb = top + plotH
  const xs = chart.series.flatMap((s) => s.points.map((p) => p[0]))
  const ys = chart.series.flatMap((s) => s.points.map((p) => p[1]))
  const xMax = chart.xMax ?? Math.max(...xs)
  const yMin = chart.yMin ?? 0
  const yMax = chart.yMax ?? niceMax(Math.max(...ys, chart.threshold?.value ?? 0))
  const sx = (v) => x0 + (v / xMax) * (x1 - x0)
  const sy = (v) => yb - ((v - yMin) / (yMax - yMin)) * plotH
  const out = []
  for (const t of ticksLinear(yMax - yMin, chart.yTicks ?? 5).map((t) => t + yMin)) {
    out.push(`<line class="grid" x1="${x0}" y1="${sy(t)}" x2="${x1}" y2="${sy(t)}"/>`)
    out.push(
      `<text class="t3" x="${x0 - 8}" y="${sy(t) + 4}" font-size="12" text-anchor="end">${esc(fmtTick(t, chart.yUnit))}</text>`,
    )
  }
  for (const t of chart.xTicks ?? []) {
    out.push(
      `<text class="t3" x="${sx(t)}" y="${yb + 20}" font-size="12" text-anchor="middle">${t}</text>`,
    )
  }
  out.push(`<line class="axis" x1="${x0}" y1="${yb}" x2="${x1}" y2="${yb}"/>`)
  if (chart.threshold) {
    const ty = sy(chart.threshold.value)
    out.push(`<line class="thr" x1="${x0}" y1="${ty}" x2="${x1}" y2="${ty}"/>`)
    out.push(
      `<text class="t2" x="${x0 + 6}" y="${ty - 7}" font-size="12">${esc(chart.threshold.label)}</text>`,
    )
  }
  for (const s of chart.series) {
    const d = s.points
      .map((p, i) => `${i ? 'L' : 'M'}${sx(p[0]).toFixed(1)},${sy(p[1]).toFixed(1)}`)
      .join('')
    out.push(
      `<path class="l${s.slot}" d="${d}" fill="none" stroke-width="2" stroke-linejoin="round"/>`,
    )
    for (const p of s.points) {
      out.push(
        `<circle class="s${s.slot} ring" cx="${sx(p[0]).toFixed(1)}" cy="${sy(p[1]).toFixed(1)}" r="4.5" stroke-width="2"><title>${esc(`${s.name} · ${p[0]} ${chart.xName ?? ''}: ${p[1]}${chart.yUnit ?? ''}`)}</title></circle>`,
      )
    }
  }
  // Direct labels at the line ends, nudged apart so they do not collide.
  const ends = chart.series
    .filter((s) => !s.labelAboveLeft)
    .map((s) => ({ s, p: s.points[s.points.length - 1] }))
    .map((e) => ({ ...e, y: sy(e.p[1]) }))
    .sort((a, b) => a.y - b.y)
  for (let i = 1; i < ends.length; i++)
    if (ends[i].y - ends[i - 1].y < 16) ends[i].y = ends[i - 1].y + 16
  if (chart.threshold) {
    // Keep end labels off the dashed threshold rule.
    const ty = sy(chart.threshold.value)
    for (const e of ends) if (Math.abs(e.y - ty) < 10) e.y = e.y <= ty ? ty - 12 : ty + 16
  }
  for (const s of chart.series.filter((s) => s.labelAboveLeft))
    ends.push({ s, p: s.points[s.points.length - 1] })
  for (const e of ends) {
    // `labelAboveLeft` puts a label that would land among other lines above and left of its end point.
    const [x, y, anchor] = e.s.labelAboveLeft
      ? [sx(e.p[0]) - 8, sy(e.p[1]) - 10, 'end']
      : [sx(e.p[0]) + 10, e.y + 4, 'start']
    out.push(
      `<text class="t1" x="${x}" y="${y}" font-size="13" font-weight="600" text-anchor="${anchor}">${esc(e.s.short ?? e.s.name)}</text>`,
    )
  }
  let bottom = yb + 20
  if (chart.xLabel) {
    bottom += 20
    out.push(
      `<text class="t2" x="${(x0 + x1) / 2}" y="${bottom}" font-size="13" text-anchor="middle">${esc(chart.xLabel)}</text>`,
    )
  }
  if (chart.yLabel) {
    out.push(
      `<text class="t2" x="${PAD.left}" y="${top - 12}" font-size="13">${esc(chart.yLabel)}</text>`,
    )
  }
  const foot = footer(chart, bottom + 10)
  return finish(chart, [head.svg, leg.svg, out.join(''), foot.svg], foot.bottom + 18)
}

function finish(chart, parts, height) {
  const slots = [...new Set(chart.series.map((s) => s.slot))].sort()
  const h = Math.ceil(height)
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${h}" width="${W}" height="${h}" role="img" aria-label="${esc(chart.title)}">
<title>${esc(chart.title)}</title>
${style(slots)}
<rect class="bg" width="${W}" height="${h}" rx="8"/>
${parts.join('\n')}
</svg>
`
}

export function renderChart(chart) {
  if (chart.kind === 'hbar') return renderHbar(chart)
  if (chart.kind === 'line') return renderLine(chart)
  throw new Error(`unknown chart kind: ${chart.kind}`)
}

function main() {
  const [specPath, outDir] = process.argv.slice(2)
  if (!specPath || !outDir) {
    console.error('usage: perf-charts.mjs <charts.json> <out-dir>')
    process.exit(2)
  }
  const spec = JSON.parse(readFileSync(specPath, 'utf8'))
  mkdirSync(outDir, { recursive: true })
  for (const chart of spec.charts) {
    const file = join(outDir, `${chart.id}.svg`)
    writeFileSync(file, renderChart(chart))
    console.log(file)
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main()
