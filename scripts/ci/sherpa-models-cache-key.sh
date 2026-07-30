#!/usr/bin/env bash
# Fingerprint for CI English Sherpa STT/TTS model dirs used by integration E2E.
# Must stay aligned with export-sherpa-local-models.sh defaults + download-*:en.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

STT_ID="${SHERPA_STT_CACHE_ID:-en}"
TTS_ID="${SHERPA_TTS_CACHE_ID:-en}"

{
  printf 'stt_id=%s\n' "$STT_ID"
  printf 'tts_id=%s\n' "$TTS_ID"
  printf '%s\n' \
    examples/shared/sherpa-local-model-catalog.json \
    examples/shared/sherpa-tts-model-catalog.json \
    examples/voice-agent-local-sherpa/package.json \
    examples/voice-agent-local-sherpa/scripts/download-stt.mjs \
    examples/voice-agent-local-sherpa/scripts/download-tts.mjs \
    examples/voice-agent-local-sherpa/scripts/sherpa-tts-model-catalog.mjs \
    scripts/export-sherpa-local-models.sh \
    scripts/ci/sherpa-models-cache-key.sh \
    scripts/ci/ensure-sherpa-models.sh \
    scripts/ci/validate-sherpa-model-dirs.sh
} | LC_ALL=C sort -u | while IFS= read -r path; do
  if [[ "$path" == stt_id=* || "$path" == tts_id=* ]]; then
    printf '%s\0' "$path"
    continue
  fi
  [[ -n "$path" && -f "$path" ]] || continue
  printf '%s\0' "$path"
  cat "$path"
done | shasum -a 256 | awk '{print $1}'
