#!/usr/bin/env bash
# Runs the node:test suites of the vendor bench CLI (benchmarks/scripts). Discovered by
# run-ci-shell-tests.sh (scripts/ci/*.test.sh), which run-pr-quality.sh calls.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
node --test \
  benchmarks/scripts/env-file.test.mjs \
  benchmarks/scripts/vendors.test.mjs \
  benchmarks/scripts/environment.test.mjs \
  benchmarks/scripts/bench-vendors.test.mjs \
  benchmarks/scripts/compare.test.mjs \
  benchmarks/scripts/transport-rule.test.mjs \
  benchmarks/scripts/methodology.test.mjs
