# Speaker gate false-accept bench per voice pair (chunk 1b, plan 2026-11-13-speaker-split-other-to-caller)

Measurement only: no gate code changed. Decision g: the split work waits for a gate change that
cuts `libritts_r`-class false accepts without failing any airport caller window.

Reproduce (Tocumen bed CC0, decoded to 48 kHz mono f32le as in the airport demo README; airport
scenes A (`scene-gap4`) and B (`scene`) from the demo's `build_scene.py`, converted with
`ffmpeg -ac 1 -ar 16000 -sample_fmt s16`; never committed):

```text
M=examples/voice-agent-local-sherpa/.models
cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example false_accept_bench -- \
  --voices-root $M --speaker-dir $M/speaker --bed <tocumen48.f32> --cache <dir> \
  --airport-a <scene A 16k wav> --airport-b <scene B 16k wav> --bed-db -12 --out bench.md
```

Run 2026-11-13 on an Apple laptop, one thread per model (TTS clips from the cache of the first run). Window counts: ~66 1.5 s windows per
voice and condition (20 sentences), so one pair cell is about 1.5 % per window.

## Findings

- **Today's gate (TitaNet-small, 1.5 s, raw cosine 0.459):** pooled test FAR 3.6 % (bed) / 3.2 %
  (clean), test FRR 0 %. The libritts_r class leaks **both ways**:
  - amy-low ← libritts_r 120: **51.5 / 51.5 %** (the product's caller voice against the bystander
    that leaks in the split bench, decision g);
  - libritts_r 120 ← amy-low: **42.9 %** (bed) / 4.8 % (clean);
  - libritts_r 300 ← lessac: 0 % (bed) / **18.3 %** (clean);
  - lessac ← hfc_female: 9.9 / 15.5 %.
  Every other pair is at most 5.6 % (hfc_female ← lessac, clean).
- **Longer windows at 0.459** make it worse (pooled bed FAR 4.2 / 5.3 / 5.9 % at 2.0 / 2.5 / 3.0 s):
  a longer window raises impostor and genuine scores together.
- **Longer windows with a tune-matched threshold** (0.692 at 2.0 s, 0.729 at 2.5 s, 0.726 at 3.0 s)
  reach 0 % test FAR but **fail airport scene B** even on the reproduced scores (0.56–0.67): the
  synthetic tune voices allow a threshold that caller audio under the bed does not reach.
- **Score normalization (cohort = 30 libritts_r speakers outside both sets)** brings every
  libritts_r pair to 0–1.2 %, but:
  - s-norm alone raises lessac ← hfc_female to 42.3 % (bed), and at the **logged** airport raw
    scores it fails scene B window 5 (2.606 < 2.648);
  - as-norm (top 10) alone raises lessac ← hfc_female to 39.4 % (bed);
  - FRR rises for the libritts_r callers only: libritts_r 300 19.7 / 25.8 % (s-norm), 15.2 /
    22.7 % (as-norm); libritts_r 120 13.6 / 3.0 % (as-norm).
- **Raw AND normalized** (accept only when raw >= 0.459 **and** the normalized score >= its
  tune-matched threshold) keeps the libritts_r fix and removes the new lessac ← hfc_female rise
  (9.9 / 8.5 % s-norm, 9.9 / 9.9 % as-norm, vs 9.9 / 15.5 % today). Pooled test FAR 0.4 / 0.4 %
  (s-norm) and 0.3 / 0.4 % (as-norm). FRR is the same as the normalized score alone (the raw
  condition rejects no genuine window).
- **Cohort confound:** the cohort is all libritts_r, the same TTS family as the leaking voices.
  Window-side cohort means (TitaNet 1.5 s, table below) are highest for the libritts_r voices
  (μt 0.30 / 0.37 for libritts_r 120, 0.36 / 0.43 for 300, bed / clean) and lowest for joe
  (0.07 / 0.02). So the normalization pushes libritts_r windows down because the cohort is their
  own family, which explains both the 0 % FAR against them and the higher FRR when they are the
  caller. **`sherpa-model-catalog.mjs` lists no other multi-speaker TTS** (its Piper list is
  ryan, kristin, hfc_male, joe, hfc_female, lessac and libritts_r), so a cross-family cohort
  could not be built from documented, script-fetched assets. Whether the gain holds against a
  real bystander who is not in the cohort's family is not shown by this bench.
- **Other models** (`3dspeaker_speech_campplus_sv_en_voxceleb_16k`, `wespeaker_en_voxceleb_resnet34_LM`,
  both in `sherpa-model-catalog.mjs` and on disk under `.models/speaker`): far worse on these
  voices at a tune-matched threshold (campplus 68 % FAR, resnet34 34 % FAR, resnet34 85 ms per
  embed). Not candidates.
- **Airport reproduction:** enrollment without pre-roll (as the gate). The offline gate
  reproduces the logged TitaNet scores with RMSE 0.048 (A) and 0.056 (B) after fitting the
  utterance start (22.5 s / 36.4 s). The reproduced raw scores run high (B 0.587 / 0.606 / 0.557
  vs 0.515 / 0.514 / 0.562 logged), so each 1.5 s candidate is also scored with the logged raw
  cosine; `airport pass` needs both.

## Recommendation

**TitaNet-small, 1.5 s windows, accept only when raw cosine >= 0.459 AND the as-norm score
(top 10 of a fixed 30-speaker cohort) >= 2.672 (tune-matched).** It is the only 1.5 s candidate
that passes every airport caller window at both the reproduced and the logged raw scores, and it
cuts the libritts_r-class false accepts in both directions (amy-low ← libritts_r 120 51.5 → 0 %,
libritts_r 120 ← amy-low 42.9 → 0 %, libritts_r 300 ← lessac 18.3 → 0 %) without raising any
other pair (lessac ← hfc_female 9.9 / 15.5 → 9.9 / 9.9 %). Pooled test FAR 3.6 / 3.2 → 0.3 /
0.4 %. Latency: no extra window time and no extra embed; per window it adds 30 cosine products
and a top-10 sort (the enrollment side once), negligible next to the 17.3 / 20.3 ms (p50 / p99)
TitaNet embed.

This is a weak recommendation. Open points for the parent / user before chunk 2:
1. **Airport margin is thin:** scene B at the logged raw scores is 2.707 / 2.687 / 3.215 against
   2.672 (0.015 above on window 6). One more real-world dB of noise could fail it.
2. **Cohort confound (above):** the libritts_r gain may be an artefact of an in-family cohort.
   No other multi-speaker TTS is in our catalog, so a cross-family cohort was not measured.
3. **FRR cost on libritts_r callers:** 13.6 / 3.0 % (120) and 15.2 / 22.7 % (300) per window,
   vs 0 % today. The re-check can still confirm on a later window, but this is a real cost.
4. **Cohort asset:** the 30 cohort embeddings must ship with the gate (an asset next to the
   model, or embedded constants). A cohort of real recordings would be better than Piper voices.
5. Everything except the airport demo is Piper TTS. Thresholds should be re-checked on real
   speech before release.

## Bench output

Bed: Tocumen airport CC0 at -12 dB re the voice (`bed`), or none (`clean`). Voices at -20 dBFS. Test callers/others: amy-low, joe, hfc_female, lessac, libritts_r 120, libritts_r 300. Tune: ryan, kristin, hfc_male, libritts_r 10, libritts_r 50. Cohort: libritts_r sids [5, 15, 30, 70, 90, 110, 140, 160, 180, 200, 220, 240, 260, 280, 320, 340, 360, 380, 400, 420, 450, 500, 550, 600, 650, 700, 750, 800, 850, 900]. Thresholds: `@0.459` fixed, else tune-matched (tune FRR <= 1.0 %).

### Summary per candidate

| candidate | threshold | test FAR bed / clean % | worst pair FAR bed % | FAR, other = libritts_r bed / clean % | test FRR bed / clean % | tune FAR / FRR % (pooled) | airport A w9, w10 | airport B w5, w6, w7 | airport A / B at logged raw | pass reproduced / at logged raw | airport pass | embed ms p50 / p99 | extra confirm delay s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| titanet 1.5s raw @0.459 | 0.459 | 3.6 / 3.2 | 51.5 (amy-low ← libritts_r 120) | 5.2 / 5.6 | 0.0 / 0.0 | 5.5 / 0.0 | 0.744, 0.714 | 0.587, 0.606, 0.557 | 0.697, 0.650 / 0.515, 0.514, 0.562 | yes / yes | yes | 17.3 / 20.3 | 0.0 |
| titanet 2s raw @0.459 | 0.459 | 4.2 / 4.1 | 58.0 (amy-low ← libritts_r 120) | 5.9 / 6.1 | 0.0 / 0.0 | 6.2 / 0.0 | 0.742, 0.770 | 0.593, 0.653, 0.618 | - | yes / - | yes | 21.3 / 25.8 | 0.5 |
| titanet 2.5s raw @0.459 | 0.459 | 5.3 / 4.1 | 80.0 (amy-low ← libritts_r 120) | 8.1 / 7.3 | 0.0 / 0.0 | 7.2 / 0.0 | 0.736, 0.776 | 0.578, 0.642, 0.666 | - | yes / - | yes | 25.6 / 32.5 | 1.0 |
| titanet 3s raw @0.459 | 0.459 | 5.9 / 4.0 | 87.9 (amy-low ← libritts_r 120) | 8.8 / 7.0 | 0.0 / 0.0 | 7.6 / 0.0 | 0.727, 0.781 | 0.591, 0.656, 0.664 | - | yes / - | yes | 29.7 / 38.1 | 1.5 |
| titanet 2s raw | 0.692 | 0.0 / 0.0 | 0.0 () | 0.0 / 0.0 | 0.9 / 0.3 | 0.0 / 1.0 | 0.742, 0.770 | 0.593, 0.653, 0.618 | - | **no** / - | **no** | 21.3 / 25.8 | 0.5 |
| titanet 2.5s raw | 0.729 | 0.0 / 0.0 | 0.0 () | 0.0 / 0.0 | 2.0 / 0.4 | 0.0 / 1.0 | 0.736, 0.776 | 0.578, 0.642, 0.666 | - | **no** / - | **no** | 25.6 / 32.5 | 1.0 |
| titanet 3s raw | 0.726 | 0.0 / 0.0 | 0.0 () | 0.0 / 0.0 | 0.5 / 0.0 | 0.0 / 0.9 | 0.727, 0.781 | 0.591, 0.656, 0.664 | - | **no** / - | **no** | 29.7 / 38.1 | 1.5 |
| titanet 1.5s s-norm | 2.648 | 1.9 / 0.4 | 42.3 (lessac ← hfc_female) | 0.0 / 0.0 | 3.3 / 4.0 | 1.3 / 0.9 | 5.278, 4.868 | 3.187, 3.538, 2.993 | 4.857, 4.308 / 2.606, 2.749, 3.038 | yes / **no** | **no** | 17.3 / 20.3 | 0.0 |
| titanet 1.5s as-norm | 2.672 | 1.5 / 0.5 | 39.4 (lessac ← hfc_female) | 0.0 / 0.0 | 4.4 / 4.0 | 2.2 / 0.9 | 6.847, 5.795 | 3.879, 4.056, 3.137 | 6.131, 4.905 / 2.707, 2.687, 3.215 | yes / yes | yes | 17.3 / 20.3 | 0.0 |
| titanet 1.5s raw AND s-norm | 2.648 | 0.4 / 0.4 | 9.9 (lessac ← hfc_female) | 0.0 / 0.0 | 3.3 / 4.0 | 0.8 / 0.9 | 5.278, 4.868 | 3.187, 3.538, 2.993 | 4.857, 4.308 / 2.606, 2.749, 3.038 | yes / **no** | **no** | 17.3 / 20.3 | 0.0 |
| titanet 1.5s raw AND as-norm | 2.672 | 0.3 / 0.4 | 9.9 (lessac ← hfc_female) | 0.0 / 0.0 | 4.4 / 4.0 | 1.0 / 0.9 | 6.847, 5.795 | 3.879, 4.056, 3.137 | 6.131, 4.905 / 2.707, 2.687, 3.215 | yes / yes | yes | 17.3 / 20.3 | 0.0 |
| titanet 2s s-norm | 2.852 | 1.5 / 0.2 | 37.7 (lessac ← hfc_female) | 0.0 / 0.0 | 2.8 / 4.7 | 0.6 / 1.0 | 5.203, 5.288 | 3.170, 3.887, 3.335 | - | yes / - | yes | 21.3 / 25.8 | 0.5 |
| titanet 2s as-norm | 3.035 | 1.4 / 0.4 | 34.0 (lessac ← hfc_female) | 0.0 / 0.0 | 7.2 / 5.9 | 1.1 / 1.0 | 6.295, 6.481 | 3.964, 4.805, 3.690 | - | yes / - | yes | 21.3 / 25.8 | 0.5 |
| titanet 2s raw AND s-norm | 2.852 | 0.3 / 0.2 | 9.4 (lessac ← hfc_female) | 0.0 / 0.0 | 2.8 / 4.7 | 0.4 / 1.0 | 5.203, 5.288 | 3.170, 3.887, 3.335 | - | yes / - | yes | 21.3 / 25.8 | 0.5 |
| titanet 2s raw AND as-norm | 3.035 | 0.3 / 0.4 | 9.4 (lessac ← hfc_female) | 0.0 / 0.0 | 7.2 / 5.9 | 0.7 / 1.0 | 6.295, 6.481 | 3.964, 4.805, 3.690 | - | yes / - | yes | 21.3 / 25.8 | 0.5 |
| campplus 1.5s raw | 0.110 | 68.1 / 66.6 | 100.0 (amy-low ← hfc_female) | 79.7 / 78.3 | 3.0 / 1.4 | 85.1 / 0.9 | 0.186, 0.082 | 0.151, 0.065, 0.328 | - | **no** / - | **no** | 25.4 / 29.5 | 0.0 |
| campplus 1.5s as-norm | -15.191 | 91.2 / 75.1 | 100.0 (amy-low ← hfc_female) | 95.9 / 81.4 | 0.0 / 15.2 | 96.4 / 0.9 | -11.072, -13.215 | -11.866, -14.072, -8.540 | - | yes / - | yes | 25.4 / 29.5 | 0.0 |
| resnet34 1.5s raw | 0.591 | 34.1 / 16.5 | 100.0 (lessac ← hfc_female) | 30.9 / 11.4 | 3.3 / 0.2 | 33.2 / 0.9 | 0.830, 0.831 | 0.730, 0.812, 0.730 | - | yes / - | yes | 84.8 / 91.8 | 0.0 |
| resnet34 1.5s as-norm | -2.059 | 22.2 / 26.2 | 98.8 (libritts_r 120 ← amy-low) | 11.5 / 17.4 | 4.2 / 0.0 | 21.3 / 0.9 | 2.242, 2.000 | 0.727, 2.067, 0.677 | - | yes / - | yes | 84.8 / 91.8 | 0.0 |

### FAR per pair (caller ← other), bed / clean %

| pair | titanet 1.5s raw @0.459 | titanet 2s raw @0.459 | titanet 2.5s raw @0.459 | titanet 3s raw @0.459 | titanet 2s raw | titanet 2.5s raw | titanet 3s raw | titanet 1.5s s-norm | titanet 1.5s as-norm | titanet 1.5s raw AND s-norm | titanet 1.5s raw AND as-norm | titanet 2s s-norm | titanet 2s as-norm | titanet 2s raw AND s-norm | titanet 2s raw AND as-norm | campplus 1.5s raw | campplus 1.5s as-norm | resnet34 1.5s raw | resnet34 1.5s as-norm |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| amy-low ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 1.4 | 4.2 / 1.4 | 0.0 / 0.0 | 0.0 / 0.0 |
| amy-low ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 9.9 / 4.2 | 15.5 / 57.7 |
| amy-low ← lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| amy-low ← libritts_r 120 | 51.5 / 51.5 | 58.0 / 58.0 | 80.0 / 72.5 | 87.9 / 69.7 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 31.8 / 59.1 | 34.8 / 80.3 |
| amy-low ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 69.7 / 84.8 | 69.7 / 57.6 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 46.4 / 53.6 | 94.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 98.6 / 100.0 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 74.6 / 83.1 | 85.9 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 100.0 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 84.8 / 72.7 | 93.9 / 97.0 | 66.7 / 3.0 | 3.0 / 0.0 |
| hfc_female ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 20.2 / 19.0 | 97.6 / 100.0 | 59.5 / 29.8 | 58.3 / 82.1 |
| hfc_female ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 1.4 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| hfc_female ← lessac | 1.4 / 5.6 | 0.0 / 20.8 | 4.8 / 11.9 | 2.9 / 20.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 12.7 / 4.2 | 5.6 / 4.2 | 1.4 / 2.8 | 0.0 / 2.8 | 7.5 / 1.9 | 7.5 / 5.7 | 0.0 / 1.9 | 0.0 / 5.7 | 100.0 / 100.0 | 100.0 / 100.0 | 76.1 / 57.7 | 81.7 / 98.6 |
| hfc_female ← libritts_r 120 | 0.0 / 3.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 90.9 / 93.9 | 100.0 / 100.0 | 22.7 / 50.0 | 30.3 / 80.3 |
| hfc_female ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 12.1 / 12.1 | 95.5 / 89.4 | 0.0 / 0.0 | 0.0 / 0.0 |
| lessac ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 46.4 / 50.0 | 100.0 / 100.0 | 61.9 / 0.0 | 39.3 / 0.0 |
| lessac ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.8 / 2.8 | 100.0 / 98.6 | 4.2 / 0.0 | 0.0 / 0.0 |
| lessac ← hfc_female | 9.9 / 15.5 | 9.4 / 15.1 | 11.6 / 14.0 | 14.3 / 14.3 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 42.3 / 8.5 | 39.4 / 11.3 | 9.9 / 8.5 | 9.9 / 9.9 | 37.7 / 5.7 | 34.0 / 5.7 | 9.4 / 5.7 | 9.4 / 5.7 | 100.0 / 100.0 | 100.0 / 100.0 | 100.0 / 19.7 | 97.2 / 85.9 |
| lessac ← libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 100.0 | 100.0 / 100.0 | 68.2 / 0.0 | 40.9 / 12.1 |
| lessac ← libritts_r 300 | 0.0 / 1.5 | 0.0 / 2.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 51.5 / 36.4 | 100.0 / 81.8 | 83.3 / 0.0 | 6.1 / 1.5 |
| libritts_r 120 ← amy-low | 42.9 / 4.8 | 52.4 / 4.8 | 58.0 / 10.0 | 66.7 / 9.5 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 1.2 / 0.0 | 0.0 / 0.0 | 1.2 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 77.4 / 63.1 | 98.8 / 79.8 | 98.8 / 100.0 | 98.8 / 100.0 |
| libritts_r 120 ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 23.9 / 9.9 | 90.1 / 52.1 | 4.2 / 0.0 | 1.4 / 0.0 |
| libritts_r 120 ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 95.8 / 84.5 | 93.0 / 97.2 |
| libritts_r 120 ← lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 43.7 / 2.8 | 29.6 / 36.6 |
| libritts_r 120 ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 86.4 | 100.0 / 84.8 | 24.2 / 1.5 | 0.0 / 0.0 |
| libritts_r 300 ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 71.4 / 56.0 | 85.7 / 0.0 | 9.5 / 0.0 | 0.0 / 0.0 |
| libritts_r 300 ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 7.0 / 1.4 | 21.1 / 0.0 | 87.3 / 40.8 | 9.9 / 1.4 |
| libritts_r 300 ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 98.6 | 100.0 / 9.9 | 9.9 / 2.8 | 0.0 / 0.0 |
| libritts_r 300 ← lessac | 0.0 / 18.3 | 1.9 / 26.4 | 0.0 / 16.7 | 0.0 / 11.4 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 98.6 / 98.6 | 98.6 / 0.0 | 40.8 / 26.8 | 2.8 / 35.2 |
| libritts_r 300 ← libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 97.0 | 100.0 / 3.0 | 12.1 / 0.0 | 0.0 / 0.0 |

### FRR per caller, bed / clean %

| caller | titanet 1.5s raw @0.459 | titanet 2s raw @0.459 | titanet 2.5s raw @0.459 | titanet 3s raw @0.459 | titanet 2s raw | titanet 2.5s raw | titanet 3s raw | titanet 1.5s s-norm | titanet 1.5s as-norm | titanet 1.5s raw AND s-norm | titanet 1.5s raw AND as-norm | titanet 2s s-norm | titanet 2s as-norm | titanet 2s raw AND s-norm | titanet 2s raw AND as-norm | campplus 1.5s raw | campplus 1.5s as-norm | resnet34 1.5s raw | resnet34 1.5s as-norm |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 14.3 / 3.6 | 0.0 / 0.0 | 0.0 / 0.0 | 1.2 / 0.0 |
| joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 3.8 / 1.9 | 4.8 / 2.4 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 11.3 / 0.0 | 12.7 / 0.0 |
| hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 8.5 / 0.0 | 8.5 / 0.0 |
| lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.0 / 0.0 | 2.5 / 0.0 | 0.0 / 0.0 | 1.5 / 0.0 | 13.6 / 3.0 | 1.5 / 0.0 | 13.6 / 3.0 | 0.0 / 0.0 | 32.0 / 8.0 | 0.0 / 0.0 | 32.0 / 8.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 1.5 | 0.0 / 0.0 |
| libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.6 / 0.0 | 3.0 / 0.0 | 19.7 / 25.8 | 15.2 / 22.7 | 19.7 / 25.8 | 15.2 / 22.7 | 18.4 / 30.6 | 14.3 / 30.6 | 18.4 / 30.6 | 14.3 / 30.6 | 1.5 / 4.5 | 0.0 / 98.5 | 0.0 / 0.0 | 3.0 / 0.0 |

### Cohort statistics per voice (TitaNet 1.5 s, libritts_r cohort)

Mean over the voice's windows of the window-side cohort mean / std (`μt` / `σt`), and the enrollment-side cohort mean / std (`μe` / `σe`).

| voice | μt / σt bed | μt / σt clean | μe / σe bed | μe / σe clean |
|---|---|---|---|---|
| amy-low | 0.225 / 0.114 | 0.162 / 0.102 | 0.271 / 0.119 | 0.242 / 0.110 |
| joe | 0.071 / 0.082 | 0.022 / 0.073 | 0.097 / 0.088 | 0.024 / 0.074 |
| hfc_female | 0.161 / 0.073 | 0.272 / 0.070 | 0.184 / 0.082 | 0.274 / 0.082 |
| lessac | 0.246 / 0.082 | 0.276 / 0.078 | 0.217 / 0.080 | 0.305 / 0.074 |
| libritts_r 120 | 0.299 / 0.125 | 0.372 / 0.108 | 0.306 / 0.125 | 0.414 / 0.111 |
| libritts_r 300 | 0.357 / 0.134 | 0.426 / 0.129 | 0.362 / 0.142 | 0.446 / 0.138 |

### Airport fit (today's gate reproduced offline)

| utterance | fitted start ms | RMSE vs log | reproduced 1.5 s scores |
|---|---|---|---|
| A utt-12 (key 3) | 22500 | 0.048 | -0.039, -0.009, 0.005, -0.072, -0.058, 0.033, 0.158, 0.245, 0.744, 0.714 |
| B utt-10 (key 4) | 36400 | 0.056 | 0.084, -0.023, -0.011, 0.005, 0.587, 0.606, 0.557 |
