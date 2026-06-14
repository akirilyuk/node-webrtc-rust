#!/usr/bin/env bash
# Download Sherpa ONNX models for local voice examples and print env exports.
#
# Full platform catalog (all STT/TTS options we offer):
#   bash scripts/make-sherpa.sh
#   npm run make-sherpa
#
# English-only (faster, matches CI):
#   bash scripts/make-sherpa.sh --minimal
#
# After download, load defaults into your shell:
#   source scripts/export-sherpa-local-models.sh
#
# Override active bundle (models must exist under .models/):
#   SHERPA_STT_BUNDLE=sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06 \
#   SHERPA_TTS_BUNDLE=vits-piper-de_DE-thorsten-medium \
#   SHERPA_STT_LANGUAGE=de source scripts/export-sherpa-local-models.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WS="@node-webrtc-rust/example-voice-agent-local-sherpa"

if [[ "${1:-}" == "--print-env" ]]; then
  exec bash "${ROOT}/scripts/export-sherpa-local-models.sh"
fi

for cmd in curl tar node; do
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "make-sherpa: install ${cmd}" >&2
    exit 1
  }
done

cd "${ROOT}"

if [[ "${1:-}" == "--minimal" ]]; then
  npm run download-all:minimal --workspace="${WS}"
else
  npm run download-all --workspace="${WS}"
fi

echo ""
echo "make-sherpa: default English env vars:"
bash "${ROOT}/scripts/export-sherpa-local-models.sh"
