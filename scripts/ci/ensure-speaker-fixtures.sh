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
# rejects the caller). --babble-clear-pull moves such a sentence to start just before that 1 s
# window instead of after the caller turn (or dropping it): dropped sentences left babble-free holes,
# the adaptive VAD noise floor (70th percentile over 3 s) fell to silence, and the babble plus the
# next bystander turn then held the VAD open into the next caller turn. --long-bystanders 2 gives
# two bystander turns per scene a 17-19 word sentence (4-6 s), so at least one bystander per pass
# stays VAD-open past the speaker gate's 1500 ms minimum check (short 7-10 word sentences often
# ended as unknown/short in both passes: vacuous pass). Scenes are never committed.
#
# Idempotent: skips when manifest.json exists and .speaker-fixtures-args (written after a successful
# generation) matches the generator args. The speaker model is downloaded only when missing (the CI
# test image ships it under /opt/sherpa-models/speaker, linked by ensure-sherpa-models.sh).
#
# SPEAKER_FIXTURES_REROLL=1 (sherpa-e2e-stress.yml input reroll_fixtures): ignore the stamp and
# generate with a fresh, empty clip cache, so every run (every stress iteration) plays a new Piper
# roll, as fresh CI containers do. Once per process tree: the call exports
# SPEAKER_FIXTURES_REROLLED=1, so the second call in the same iteration (run-sherpa-example-ci.sh,
# then run-sherpa-roundtrip-e2e.sh) reuses that roll. The stamp is removed, not written, so the next
# normal run regenerates.
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
    --main-levels -26 --min-turn-gap-ms 2500 --bystander-db -15 --babble-clear-ms 1000 --babble-clear-pull --long-bystanders 2
    --only s3_test_m15
  )
  local want="${args[*]}"

  local reroll="${SPEAKER_FIXTURES_REROLL:-0}"
  if [[ "$reroll" == "1" && "${SPEAKER_FIXTURES_REROLLED:-0}" == "1" && -s "$out/manifest.json" ]]; then
    echo "==> noisy-bystander scenes already re-rolled in this run ($out)"
    return 0
  fi
  if [[ "$reroll" != "1" && -s "$out/manifest.json" && -f "$stamp" && "$(cat "$stamp")" == "$want" ]]; then
    echo "==> noisy-bystander scenes up to date ($out)"
    return 0
  fi

  echo "==> speaker assets + noisy-bystander scenes"
  if [[ ! -s "$ex/.models/speaker/nemo_en_titanet_small.onnx" ]]; then
    bash "$ci_step" --timeout "$timeout" \
      "sherpa download-speaker" -- npm run download-speaker --workspace="$workspace" || return 1
  fi
  rm -f "$stamp"
  local cache_dir="" rc=0
  if [[ "$reroll" == "1" ]]; then
    cache_dir="$(mktemp -d)"
    args+=(--cache "$cache_dir")
    echo "==> SPEAKER_FIXTURES_REROLL=1: fresh Piper roll (empty cache $cache_dir)"
  fi
  bash "$ci_step" --timeout "$timeout" \
    "sherpa speaker_fixtures" -- \
    cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- "${args[@]}" \
    || rc=1
  [[ -n "$cache_dir" ]] && rm -rf "$cache_dir"
  [[ "$rc" -eq 0 ]] || return 1
  [[ -s "$out/manifest.json" ]] || { echo "speaker_fixtures wrote no $out/manifest.json" >&2; return 1; }
  if [[ "$reroll" == "1" ]]; then
    export SPEAKER_FIXTURES_REROLLED=1
  else
    printf '%s' "$want" >"$stamp"
  fi
}
