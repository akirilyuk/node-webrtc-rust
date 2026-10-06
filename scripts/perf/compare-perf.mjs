#!/usr/bin/env node
// Compare two perf baseline files written by scripts/perf/run-perf-baseline.sh.
//
//   node scripts/perf/compare-perf.mjs <base.json> <head.json>
//
// Prints a table (metric, base, head, delta %, band %, verdict) and exits 1 when
//   - a metric moved in the wrong direction by more than its noise band,
//   - an `equal` metric changed,
//   - a metric present in base is missing in head (guards test.vacuous-pass), or
//   - a metric in base has no entry in DIRECTIONS.
// Exit 2 on usage or unreadable input.
//
// Noise band per metric: max(10 %, 2 x spread_pct recorded for that metric in the base file).
// Perf thresholds are not PR CI gates (wall-clock on shared runners is flaky); run this by hand
// per phase and paste the table into the feature log.

import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** Direction per metric: "lower" is better, "higher" is better, "equal" must not change. */
export const DIRECTIONS = {
  // Layer A: nwr micro benches (chunk 0.1)
  inbound_frame_silence_ns: "lower",
  inbound_frame_voiced_ns: "lower",
  inbound_frame_allocs: "lower",
  tts_drain_frames_ns_per_frame: "lower",
  tts_drain_bytes_copied_per_frame: "lower",
  resample_22k_10s_ns: "lower",
  resample_stream_22k_10s_ns: "lower",
  resample_peak_bytes: "lower",
  resample_stream_peak_bytes: "lower",
  opus_encode_20ms_ns: "lower",
  idle_agent_cpu_us_per_agent_s: "lower",
  // Layer B: speech-service probes (chunk 0.2)
  stt_rtf_total: "higher",
  stt_msg_latency_ms: "lower",
  stt_cpu_s_per_audio_s: "lower",
  tts_ttfc_ms: "lower",
  tts_queue_wait_ms: "lower",
  tts_synth_wall_ms: "lower",
  rss_mb_end: "lower",
  rss_mb_growth: "lower",
  transcript_exact_match_rate: "equal",
  // Layer C: load-staging report (chunk 0.3)
  serverSttFinalMs: "lower",
  ttsToAgentStartMs: "lower",
  responseLatencyMs: "lower",
  finalToAgentStartMs: "lower",
  turnSuccessRate: "higher",
  sessionCycleErrorRate: "lower",
};

const MIN_BAND_PCT = 10;
const EQUAL_EPSILON = 1e-9;

/**
 * Pure comparison. `base` / `head` are parsed baseline files:
 * `{ metrics: { name: number }, spread_pct?: { name: number } }`.
 * Returns `{ rows, failed }`.
 */
export function compare(base, head) {
  const baseMetrics = base.metrics ?? {};
  const headMetrics = head.metrics ?? {};
  const spread = base.spread_pct ?? {};
  const rows = [];

  for (const name of Object.keys(baseMetrics).sort()) {
    const baseValue = baseMetrics[name];
    const direction = DIRECTIONS[name];
    const band = Math.max(MIN_BAND_PCT, 2 * (spread[name] ?? 0));
    const row = { name, base: baseValue, head: null, deltaPct: null, band, verdict: "ok" };

    if (!(name in headMetrics)) {
      row.verdict = "FAIL missing in head";
    } else if (direction === undefined) {
      row.head = headMetrics[name];
      row.verdict = "FAIL no direction defined";
    } else {
      const headValue = headMetrics[name];
      row.head = headValue;
      if (baseValue === 0) {
        row.deltaPct = headValue === 0 ? 0 : headValue > 0 ? Infinity : -Infinity;
      } else {
        row.deltaPct = ((headValue - baseValue) / Math.abs(baseValue)) * 100;
      }
      if (direction === "equal") {
        if (Math.abs(headValue - baseValue) > EQUAL_EPSILON) row.verdict = "FAIL changed (must be equal)";
      } else if (direction === "lower") {
        if (row.deltaPct > band) row.verdict = "FAIL regressed (lower is better)";
        else if (row.deltaPct < -band) row.verdict = "improved";
      } else if (direction === "higher") {
        if (row.deltaPct < -band) row.verdict = "FAIL regressed (higher is better)";
        else if (row.deltaPct > band) row.verdict = "improved";
      }
    }
    rows.push(row);
  }

  for (const name of Object.keys(headMetrics).sort()) {
    if (!(name in baseMetrics)) {
      rows.push({ name, base: null, head: headMetrics[name], deltaPct: null, band: null, verdict: "new" });
    }
  }

  return { rows, failed: rows.some((r) => r.verdict.startsWith("FAIL")) };
}

function fmt(value) {
  if (value === null || value === undefined) return "-";
  if (!Number.isFinite(value)) return String(value);
  return Number.isInteger(value) ? String(value) : value.toPrecision(5);
}

export function renderTable(rows) {
  const header = ["metric", "base", "head", "delta %", "band %", "verdict"];
  const body = rows.map((r) => [
    r.name,
    fmt(r.base),
    fmt(r.head),
    r.deltaPct === null ? "-" : r.deltaPct.toFixed(1),
    r.band === null ? "-" : r.band.toFixed(1),
    r.verdict,
  ]);
  const widths = header.map((h, i) => Math.max(h.length, ...body.map((row) => row[i].length)));
  const line = (cells) => cells.map((c, i) => c.padEnd(widths[i])).join("  ");
  return [line(header), line(widths.map((w) => "-".repeat(w))), ...body.map(line)].join("\n");
}

function main(argv) {
  if (argv.length !== 2) {
    console.error("usage: node scripts/perf/compare-perf.mjs <base.json> <head.json>");
    return 2;
  }
  let base;
  let head;
  try {
    base = JSON.parse(readFileSync(argv[0], "utf8"));
    head = JSON.parse(readFileSync(argv[1], "utf8"));
  } catch (error) {
    console.error(`compare-perf: cannot read input: ${error.message}`);
    return 2;
  }
  const { rows, failed } = compare(base, head);
  console.log(renderTable(rows));
  if (failed) {
    console.error("compare-perf: FAIL (see verdict column)");
    return 1;
  }
  console.log("compare-perf: ok");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exit(main(process.argv.slice(2)));
}
