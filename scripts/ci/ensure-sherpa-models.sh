#!/usr/bin/env bash
# Ensure English Sherpa model dirs are present and valid for CI E2E.
# On validation failure (corrupt/partial cache restore), wipe and redownload.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

WORKSPACE="${SHERPA_EXAMPLE_WORKSPACE:-@node-webrtc-rust/example-voice-agent-local-sherpa}"
TIMEOUT="${CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC:-900}"
CI_STEP="$ROOT/scripts/ci/ci-step.sh"
VALIDATE="$ROOT/scripts/ci/validate-sherpa-model-dirs.sh"

# Canonical default paths (do not trust ambient SHERPA_* for rm/redownload).
unset SHERPA_STT_MODEL_PATH SHERPA_TTS_MODEL_PATH SHERPA_STT_LANGUAGE
# shellcheck source=/dev/null
SHERPA_EXPORT_SKIP_VALIDATE=1 source "$ROOT/scripts/export-sherpa-local-models.sh"
STT_DIR="$SHERPA_STT_MODEL_PATH"
TTS_DIR="$SHERPA_TTS_MODEL_PATH"

if bash "$VALIDATE"; then
  echo "==> Sherpa models OK (cache or prior download)"
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    echo "redownloaded=false" >>"$GITHUB_OUTPUT"
  fi
  exit 0
fi

echo "==> Sherpa models invalid/missing — clearing and redownloading"
rm -rf "$STT_DIR" "$TTS_DIR"

bash "$CI_STEP" --timeout "$TIMEOUT" \
  "sherpa download-stt" -- npm run download-stt:en --workspace="$WORKSPACE"
bash "$CI_STEP" --timeout "$TIMEOUT" \
  "sherpa download-tts" -- npm run download-tts:en --workspace="$WORKSPACE"

bash "$VALIDATE"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  echo "redownloaded=true" >>"$GITHUB_OUTPUT"
fi
