#!/usr/bin/env bash
# Sourced helper: speaker model, Piper voices and the generated conversation scenes for
# start:roundtrip-noisy-bystander. Sourced by run-sherpa-roundtrip-e2e.sh (every path that runs the
# roundtrip: run-sherpa-example-ci.sh, sherpa-e2e-stress.yml, local runs) and by
# run-sherpa-example-ci.sh, which calls it before its roundtrip steps so the cargo build is not
# charged to a roundtrip's step cap (ci.image-asset-lockstep).
#
# Only the two S3 test scenes the roundtrip plays (-15 dB bystander, caller at the default level and
# at -26 dBFS), with every turn gap raised to 2.5 s so the agent's default end-of-turn (1.5 s
# finalize + VAD hangover) closes each turn before the next speaker starts, and no babble sentence
# starts within 1 s before a caller turn (otherwise VAD opens on babble and the 1.5 s speaker check
# rejects the caller). Scenes are never committed.
#
# Idempotent: skips when manifest.json exists and .speaker-fixtures-args (written after a successful
# generation) matches the generator args. The speaker model is downloaded only when missing (the CI
# test image ships it under /opt/sherpa-models/speaker, linked by ensure-sherpa-models.sh).
#
# Requires ROOT (repo root). Optional: CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC (default 900).

ensure_speaker_fixtures() {
  local root="${ROOT:?ROOT must be set before ensure_speaker_fixtures}"
  local ci_step="$root/scripts/ci/ci-step.sh"
  local timeout="${CI_SHERPA_MODEL_DOWNLOAD_TIMEOUT_SEC:-900}"
  local workspace="@node-webrtc-rust/example-voice-agent-local-sherpa"
  local ex="$root/examples/voice-agent-local-sherpa"
  local out="$ex/.models/speaker-fixtures"
  local stamp="$out/.speaker-fixtures-args"
  local args=(
    --models "$ex/.models/speaker" --main-tts "$ex/.models/vits-piper-en_US-amy-low"
    --phrases "$ex/stt-phrase-qualification.json" --out "$out"
    --main-levels -26 --min-turn-gap-ms 2500 --bystander-db -15 --babble-clear-ms 1000 --only s3_test_m15
  )
  local want="${args[*]}"

  if [[ -s "$out/manifest.json" && -f "$stamp" && "$(cat "$stamp")" == "$want" ]]; then
    echo "==> noisy-bystander scenes up to date ($out)"
    return 0
  fi

  echo "==> speaker assets + noisy-bystander scenes"
  if [[ ! -s "$ex/.models/speaker/nemo_en_titanet_small.onnx" ]]; then
    bash "$ci_step" --timeout "$timeout" \
      "sherpa download-speaker" -- npm run download-speaker --workspace="$workspace" || return 1
  fi
  rm -f "$stamp"
  bash "$ci_step" --timeout "$timeout" \
    "sherpa speaker_fixtures" -- \
    cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- "${args[@]}" \
    || return 1
  [[ -s "$out/manifest.json" ]] || { echo "speaker_fixtures wrote no $out/manifest.json" >&2; return 1; }
  printf '%s' "$want" >"$stamp"
}
