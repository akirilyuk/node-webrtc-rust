#!/usr/bin/env bash
# Append a concise native CI summary to GITHUB_STEP_SUMMARY (no secrets).
#
# Env (all optional except title context):
#   SUMMARY_TITLE
#   AGGREGATE_DIGEST, ALL_CACHED
#   CACHED_TARGETS_JSON, REBUILT_TARGETS_JSON (JSON arrays)
#   PRODUCER_SHA, PRODUCER_RUN_ID, PREFERENCE, FALLBACK_REASON, BUNDLE_REUSED
#   MAIN_VALIDATED, PHASE
set -euo pipefail

OUT="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
TITLE="${SUMMARY_TITLE:-Native CI}"

{
  echo "## ${TITLE}"
  echo ""
  [[ -n "${PHASE:-}" ]] && echo "- phase: \`${PHASE}\`"
  [[ -n "${AGGREGATE_DIGEST:-}" ]] && echo "- aggregate_digest: \`${AGGREGATE_DIGEST}\`"
  [[ -n "${ALL_CACHED:-}" ]] && echo "- all_cached: \`${ALL_CACHED}\`"
  [[ -n "${CACHED_TARGETS_JSON:-}" ]] && echo "- cache_hits: \`${CACHED_TARGETS_JSON}\`"
  [[ -n "${REBUILT_TARGETS_JSON:-}" ]] && echo "- rebuilt_targets: \`${REBUILT_TARGETS_JSON}\`"
  [[ -n "${BUNDLE_REUSED:-}" ]] && echo "- bundle_reused: \`${BUNDLE_REUSED}\`"
  [[ -n "${PREFERENCE:-}" ]] && echo "- preference: \`${PREFERENCE}\`"
  [[ -n "${PRODUCER_SHA:-}" ]] && echo "- producer_sha: \`${PRODUCER_SHA}\`"
  [[ -n "${PRODUCER_RUN_ID:-}" ]] && echo "- producer_run: \`${PRODUCER_RUN_ID}\`"
  [[ -n "${FALLBACK_REASON:-}" ]] && echo "- fallback_reason: \`${FALLBACK_REASON}\`"
  [[ -n "${MAIN_VALIDATED:-}" ]] && echo "- main_validated: \`${MAIN_VALIDATED}\`"
  echo ""
} >>"$OUT"
