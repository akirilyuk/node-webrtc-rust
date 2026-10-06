// Run: node --test scripts/perf/collect-perf.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";

import { robustSpreadPct } from "./collect-perf.mjs";

test("robust spread ignores one outlier", () => {
  const spread = robustSpreadPct([20628.8, 22418.1, 20619.8, 79691.3, 20744.0]);
  assert.ok(spread < 10, `spread_pct ${spread} should be below 10`);
});

test("robust spread is 0 for identical runs and for a zero median", () => {
  assert.equal(robustSpreadPct([5, 5, 5]), 0);
  assert.equal(robustSpreadPct([0, 0, 0]), 0);
});

test("robust spread still reports real scatter", () => {
  assert.ok(robustSpreadPct([100, 120, 80, 130, 70]) > 10);
});
