#!/usr/bin/env node
// Collect Layer A results into one baseline JSON (called by run-perf-baseline.sh).
//
//   node scripts/perf/collect-perf.mjs <bench-log> <criterion-dir> <label> <sha> <out.json>
//
// Sources:
//   - `perf_probe {json}` lines in <bench-log>. A probe that ran several times contributes the
//     median of each metric; its spread_pct is (max - min) / median * 100.
//   - criterion `<criterion-dir>/<metric>/new/estimates.json` (bench id == metric name): the
//     median point estimate in ns; spread_pct is the width of its 95 % confidence interval
//     relative to the median.
// Exits 1 when any expected Layer A metric is missing (a bench that silently stopped emitting
// must not produce a baseline).

import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

export const EXPECTED_METRICS = [
  "inbound_frame_silence_ns",
  "inbound_frame_voiced_ns",
  "inbound_frame_allocs",
  "tts_drain_frames_ns_per_frame",
  "tts_drain_bytes_copied_per_frame",
  "resample_22k_10s_ns",
  "resample_stream_22k_10s_ns",
  "resample_peak_bytes",
  "resample_stream_peak_bytes",
  "opus_encode_20ms_ns",
  "idle_agent_cpu_us_per_agent_s",
];

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function main(argv) {
  if (argv.length !== 5) {
    console.error("usage: collect-perf.mjs <bench-log> <criterion-dir> <label> <sha> <out.json>");
    return 2;
  }
  const [logPath, criterionDir, label, sha, outPath] = argv;

  const samples = {}; // metric -> number[]
  const probes = [];
  for (const line of readFileSync(logPath, "utf8").split("\n")) {
    const at = line.indexOf("perf_probe ");
    if (at === -1) continue;
    const probe = JSON.parse(line.slice(at + "perf_probe ".length));
    probes.push(probe);
    for (const [name, value] of Object.entries(probe.metrics)) {
      (samples[name] ??= []).push(value);
    }
  }

  const metrics = {};
  const spreadPct = {};
  for (const [name, values] of Object.entries(samples)) {
    const mid = median(values);
    metrics[name] = mid;
    spreadPct[name] = mid === 0 ? 0 : ((Math.max(...values) - Math.min(...values)) / Math.abs(mid)) * 100;
  }

  if (existsSync(criterionDir)) {
    for (const name of readdirSync(criterionDir)) {
      const estimates = join(criterionDir, name, "new", "estimates.json");
      if (!existsSync(estimates)) continue;
      const { median: m } = JSON.parse(readFileSync(estimates, "utf8"));
      metrics[name] = m.point_estimate;
      const ci = m.confidence_interval;
      spreadPct[name] = ((ci.upper_bound - ci.lower_bound) / m.point_estimate) * 100;
    }
  }

  const missing = EXPECTED_METRICS.filter((name) => !(name in metrics));
  if (missing.length > 0) {
    console.error(`collect-perf: missing Layer A metrics: ${missing.join(", ")}`);
    return 1;
  }

  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, `${JSON.stringify({ label, sha, metrics, spread_pct: spreadPct, probes }, null, 2)}\n`);
  console.log(`collect-perf: wrote ${outPath} (${Object.keys(metrics).length} metrics)`);
  return 0;
}

process.exit(main(process.argv.slice(2)));
