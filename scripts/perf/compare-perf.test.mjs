// Run: node --test scripts/perf/compare-perf.test.mjs
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { DIRECTIONS } from "./compare-perf.mjs";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "compare-perf.mjs");

/** Write base/head files and run the CLI. */
function run(base, head) {
  const dir = mkdtempSync(join(tmpdir(), "compare-perf-"));
  const basePath = join(dir, "base.json");
  const headPath = join(dir, "head.json");
  writeFileSync(basePath, JSON.stringify(base));
  writeFileSync(headPath, JSON.stringify(head));
  return spawnSync(process.execPath, [SCRIPT, basePath, headPath], { encoding: "utf8" });
}

test("improvement passes (lower metric drops, higher metric rises)", () => {
  const result = run(
    { metrics: { inbound_frame_voiced_ns: 1000, stt_rtf_total: 4 } },
    { metrics: { inbound_frame_voiced_ns: 500, stt_rtf_total: 8 } },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /improved/);
});

test("regression beyond the band exits 1 (lower metric)", () => {
  const result = run(
    { metrics: { inbound_frame_voiced_ns: 1000 } },
    { metrics: { inbound_frame_voiced_ns: 1200 } },
  );
  assert.equal(result.status, 1);
  assert.match(result.stdout, /FAIL regressed/);
});

test("regression beyond the band exits 1 (higher metric)", () => {
  const result = run({ metrics: { stt_rtf_total: 8 } }, { metrics: { stt_rtf_total: 6 } });
  assert.equal(result.status, 1);
});

test("movement inside the 10 % band passes", () => {
  const result = run(
    { metrics: { inbound_frame_voiced_ns: 1000 } },
    { metrics: { inbound_frame_voiced_ns: 1090 } },
  );
  assert.equal(result.status, 0, result.stdout);
});

test("band widens to 2 x the baseline spread", () => {
  const base = { metrics: { inbound_frame_voiced_ns: 1000 }, spread_pct: { inbound_frame_voiced_ns: 20 } };
  const within = run(base, { metrics: { inbound_frame_voiced_ns: 1350 } });
  assert.equal(within.status, 0, within.stdout);
  const beyond = run(base, { metrics: { inbound_frame_voiced_ns: 1450 } });
  assert.equal(beyond.status, 1);
});

test("a metric missing in head exits 1", () => {
  const result = run(
    { metrics: { inbound_frame_voiced_ns: 1000, opus_encode_20ms_ns: 500 } },
    { metrics: { inbound_frame_voiced_ns: 1000 } },
  );
  assert.equal(result.status, 1);
  assert.match(result.stdout, /missing in head/);
});

test("an equal metric that changed exits 1, unchanged passes", () => {
  const changed = run(
    { metrics: { transcript_exact_match_rate: 1 } },
    { metrics: { transcript_exact_match_rate: 0.99 } },
  );
  assert.equal(changed.status, 1);
  const same = run(
    { metrics: { transcript_exact_match_rate: 1 } },
    { metrics: { transcript_exact_match_rate: 1 } },
  );
  assert.equal(same.status, 0, same.stdout);
});

test("lower metric going from 0 to non-zero exits 1", () => {
  const result = run({ metrics: { inbound_frame_allocs: 0 } }, { metrics: { inbound_frame_allocs: 1 } });
  assert.equal(result.status, 1);
});

test("a base metric without a direction exits 1", () => {
  const result = run({ metrics: { not_a_known_metric: 1 } }, { metrics: { not_a_known_metric: 1 } });
  assert.equal(result.status, 1);
  assert.match(result.stdout, /no direction defined/);
});

test("a metric only in head is reported as new and does not fail", () => {
  const result = run(
    { metrics: { inbound_frame_voiced_ns: 1000 } },
    { metrics: { inbound_frame_voiced_ns: 1000, opus_encode_20ms_ns: 500 } },
  );
  assert.equal(result.status, 0, result.stdout);
  assert.match(result.stdout, /new/);
});

test("bad usage and unreadable input exit 2", () => {
  const usage = spawnSync(process.execPath, [SCRIPT], { encoding: "utf8" });
  assert.equal(usage.status, 2);
  const missing = spawnSync(process.execPath, [SCRIPT, "/nonexistent/a.json", "/nonexistent/b.json"], {
    encoding: "utf8",
  });
  assert.equal(missing.status, 2);
});

test("direction table covers every Layer A metric and the L10 / allocation metrics", () => {
  const emitted = [
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
  for (const name of emitted) {
    assert.equal(DIRECTIONS[name], "lower", `${name} must be a lower-is-better metric`);
  }
});
