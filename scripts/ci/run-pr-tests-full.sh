#!/usr/bin/env bash
# Full check suite (quality + integration) — used on main push and release.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

bash "$ROOT/scripts/ci/run-pr-quality.sh"
bash "$ROOT/scripts/ci/run-pr-integration.sh"
