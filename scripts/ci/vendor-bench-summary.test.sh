#!/usr/bin/env bash
# Runs the node:test suite of vendor-bench-summary.mjs. Discovered by run-ci-shell-tests.sh
# (scripts/ci/*.test.sh), which run-pr-quality.sh calls, so the bench summary is gated in CI.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
node --test scripts/ci/vendor-bench-summary.test.mjs
