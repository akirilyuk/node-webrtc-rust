#!/usr/bin/env bash
# Verify the canonical six release targets have complete npm platform mappings.
#
# Source of truth for triples: scripts/ci/list-release-targets.sh
# Does NOT fail on packages/bindings/index.js loader fallbacks beyond the six
# (those are intentional runtime fallbacks, not release publish targets).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
exec python3 "$ROOT/scripts/ci/native_build_contract.py" check-release-targets
