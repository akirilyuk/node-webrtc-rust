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

Run 2026-11-13 on an Apple laptop, one thread per model, 375 s. Window counts: ~66 1.5 s windows per
voice and condition (20 sentences), so one pair cell is about 1.5 % per window.

## Findings

- **Today's gate (TitaNet-small, 1.5 s, raw cosine 0.459):** pooled test FAR 3.0 % (bed) / 2.9 %
  (clean), test FRR 0 %. The false accepts are concentrated in one pair: **amy-low ← libritts_r
  120 at 48.5 % / 48.5 %** of windows, the product's caller voice against the test bystander that
  leaks in the split bench (decision g). Next: lessac ← hfc_female 8.5 / 11.3 %. Every other pair
  is at most 3 %.
- **Longer windows at 0.459** make it worse (amy-low ← libritts_r 120: 56 % at 2.0 s, 88 % at
  3.0 s): a longer window raises impostor and genuine scores together.
- **Longer windows with a tune-matched threshold** (0.687 at 2.0 s, 0.721 at 3.0 s) reach 0 % FAR
  on the test set but **fail airport scene B** (its caller windows score 0.58–0.67): the
  synthetic tune voices allow a threshold real-ish caller audio under the bed does not reach.
- **Score normalization (s-norm / as-norm, cohort = 30 libritts_r speakers outside both sets)**
  removes the libritts_r-class false accepts (0 % on every `← libritts_r` pair), keeps every
  airport caller window, and adds no window time. Costs: lessac ← hfc_female rises to 29.6 % (bed)
  / 7.0 % (clean) with s-norm, and the libritts_r 300 caller's FRR rises to 18.2 / 19.7 %
  (libritts_r 120: 1.5 / 0 %). As-norm (top 10) is worse on FRR (5.6 / 4.2 % pooled).
- **Other models** (`3dspeaker_speech_campplus_sv_en_voxceleb_16k`, `wespeaker_en_voxceleb_resnet34_LM`,
  both in `sherpa-model-catalog.mjs` and on disk under `.models/speaker`): far worse on these
  voices at a tune-matched threshold (campplus 69 % FAR, resnet34 31 % FAR, resnet34 88 ms per
  embed). Not candidates.
- **Airport reproduction:** the offline gate reproduces the logged TitaNet scores with RMSE 0.051
  (A) and 0.054 (B) after fitting the utterance start (22.5 s / 36.4 s). Scene B's caller windows
  reproduce at 0.589 / 0.601 / 0.563 vs 0.515 / 0.514 / 0.562 logged.

## Recommendation

**TitaNet-small, 1.5 s windows, s-norm against a fixed impostor cohort, threshold 2.62
(tune-matched).** It is the only candidate that cuts the libritts_r-class false accepts (amy-low ←
libritts_r 120: 48.5 % → 0 %; pooled test FAR 3.0 → 1.3 % bed, 2.9 → 0.3 % clean) and passes every
airport caller window (A 5.39 / 4.83, B 3.21 / 3.51 / 3.05 vs 2.62). Latency: no extra window
time and no extra embed; per window it adds 30 cosine products against the cohort plus the cohort
stats of the enrollment (computed once at enrollment), negligible next to the 17.8 / 19.4 ms
(p50 / p99) TitaNet embed.

Open points for the parent / user before chunk 2:
1. The cohort is 30 embeddings that must ship with the gate (an asset next to the model, or
   embedded constants). Here they come from Piper libritts_r speakers; a cohort of real
   recordings would be closer to production impostors.
2. The trade it makes: lessac ← hfc_female 8.5 → 29.6 % (bed), and FRR 0 → 18 % for the
   libritts_r 300 caller. A caller rejected on one window is not fatal with the re-check (the
   next window can still confirm), but FRR is per window here.
3. Airport scene B's smallest margin is 3.05 vs 2.62 (one window, one run).
4. Everything except the airport demo is Piper TTS. Thresholds should be re-checked on real
   speech before release.

## Bench output

Bed: Tocumen airport CC0 at -12 dB re the voice (`bed`), or none (`clean`). Voices at -20 dBFS. Test callers/others: amy-low, joe, hfc_female, lessac, libritts_r 120, libritts_r 300. Tune: ryan, kristin, hfc_male, libritts_r 10, libritts_r 50. Cohort: libritts_r sids [5, 15, 30, 70, 90, 110, 140, 160, 180, 200, 220, 240, 260, 280, 320, 340, 360, 380, 400, 420, 450, 500, 550, 600, 650, 700, 750, 800, 850, 900]. Thresholds: `@0.459` fixed, else tune-matched (tune FRR <= 1.0 %).

### Summary per candidate

| candidate | threshold | test FAR bed / clean % | worst pair FAR bed % | FAR, other = libritts_r bed / clean % | test FRR bed / clean % | tune FAR / FRR % (pooled) | airport A w9, w10 | airport B w5, w6, w7 | airport pass | embed ms p50 / p99 | extra confirm delay s |
|---|---|---|---|---|---|---|---|---|---|---|---|
| titanet 1.5s raw @0.459 | 0.459 | 3.0 / 2.9 | 48.5 (amy-low ← libritts_r 120) | 4.8 / 5.3 | 0.0 / 0.0 | 5.3 / 0.0 | 0.751, 0.706 | 0.589, 0.601, 0.563 | yes | 17.8 / 19.4 | 0.0 |
| titanet 2s raw @0.459 | 0.459 | 3.6 / 3.7 | 56.0 (amy-low ← libritts_r 120) | 5.7 / 5.9 | 0.0 / 0.0 | 6.1 / 0.0 | 0.759, 0.765 | 0.596, 0.648, 0.622 | yes | 22.1 / 23.7 | 0.5 |
| titanet 2.5s raw @0.459 | 0.459 | 4.5 / 3.9 | 72.5 (amy-low ← libritts_r 120) | 7.3 / 7.6 | 0.0 / 0.0 | 7.0 / 0.0 | 0.752, 0.774 | 0.582, 0.633, 0.670 | yes | 26.7 / 28.5 | 1.0 |
| titanet 3s raw @0.459 | 0.459 | 5.1 / 4.0 | 87.9 (amy-low ← libritts_r 120) | 8.8 / 7.3 | 0.0 / 0.0 | 7.4 / 0.0 | 0.745, 0.782 | 0.597, 0.656, 0.667 | yes | 30.8 / 32.7 | 1.5 |
| titanet 2s raw | 0.687 | 0.0 / 0.0 | 0.0 () | 0.0 / 0.0 | 0.9 / 0.3 | 0.0 / 1.0 | 0.759, 0.765 | 0.596, 0.648, 0.622 | **no** | 22.1 / 23.7 | 0.5 |
| titanet 2.5s raw | 0.718 | 0.0 / 0.0 | 0.0 () | 0.0 / 0.0 | 2.0 / 0.4 | 0.0 / 1.0 | 0.752, 0.774 | 0.582, 0.633, 0.670 | **no** | 26.7 / 28.5 | 1.0 |
| titanet 3s raw | 0.721 | 0.0 / 0.0 | 0.0 () | 0.0 / 0.0 | 0.5 / 0.0 | 0.0 / 0.9 | 0.745, 0.782 | 0.597, 0.656, 0.667 | **no** | 30.8 / 32.7 | 1.5 |
| titanet 1.5s s-norm | 2.623 | 1.3 / 0.3 | 29.6 (lessac ← hfc_female) | 0.0 / 0.0 | 3.0 / 3.0 | 1.1 / 0.9 | 5.387, 4.830 | 3.210, 3.508, 3.054 | yes | 17.8 / 19.4 | 0.0 |
| titanet 1.5s as-norm | 2.671 | 1.1 / 0.4 | 29.6 (lessac ← hfc_female) | 0.0 / 0.0 | 5.6 / 4.2 | 2.0 / 0.9 | 6.908, 5.626 | 3.850, 3.927, 3.177 | yes | 17.8 / 19.4 | 0.0 |
| titanet 2s s-norm | 2.852 | 1.1 / 0.2 | 26.4 (lessac ← hfc_female) | 0.0 / 0.0 | 2.8 / 4.7 | 0.4 / 1.0 | 5.394, 5.290 | 3.209, 3.857, 3.380 | yes | 22.1 / 23.7 | 0.5 |
| titanet 2s as-norm | 3.035 | 0.9 / 0.3 | 22.6 (lessac ← hfc_female) | 0.0 / 0.0 | 7.8 / 6.2 | 1.0 / 1.0 | 6.476, 6.360 | 3.965, 4.661, 3.686 | yes | 22.1 / 23.7 | 0.5 |
| campplus 1.5s raw | 0.110 | 69.1 / 67.3 | 100.0 (amy-low ← hfc_female) | 81.4 / 79.5 | 3.0 / 1.2 | 84.9 / 0.9 | 0.328, 0.205 | 0.185, 0.125, 0.406 | yes | 26.4 / 28.3 | 0.0 |
| campplus 1.5s as-norm | -15.191 | 91.6 / 74.2 | 100.0 (amy-low ← hfc_female) | 97.6 / 81.4 | 0.0 / 15.9 | 96.4 / 0.9 | -3.475, -6.248 | -10.619, -11.789, -5.521 | yes | 26.4 / 28.3 | 0.0 |
| resnet34 1.5s raw | 0.603 | 30.8 / 15.1 | 98.8 (libritts_r 120 ← amy-low) | 24.4 / 9.8 | 3.0 / 0.5 | 30.8 / 0.9 | 0.837, 0.841 | 0.727, 0.830, 0.749 | yes | 88.1 / 91.1 | 0.0 |
| resnet34 1.5s as-norm | -1.895 | 21.0 / 24.2 | 98.8 (libritts_r 120 ← amy-low) | 9.1 / 16.8 | 4.0 / 0.0 | 19.6 / 0.9 | 2.451, 2.233 | 0.567, 2.274, 0.964 | yes | 88.1 / 91.1 | 0.0 |

### FAR per pair (caller ← other), bed / clean %

| pair | titanet 1.5s raw @0.459 | titanet 2s raw @0.459 | titanet 2.5s raw @0.459 | titanet 3s raw @0.459 | titanet 2s raw | titanet 2.5s raw | titanet 3s raw | titanet 1.5s s-norm | titanet 1.5s as-norm | titanet 2s s-norm | titanet 2s as-norm | campplus 1.5s raw | campplus 1.5s as-norm | resnet34 1.5s raw | resnet34 1.5s as-norm |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| amy-low ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 1.4 | 18.3 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| amy-low ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 15.5 / 2.8 | 21.1 / 52.1 |
| amy-low ← lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| amy-low ← libritts_r 120 | 48.5 / 48.5 | 56.0 / 56.0 | 72.5 / 72.5 | 87.9 / 69.7 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 97.0 | 34.8 / 34.8 | 31.8 / 77.3 |
| amy-low ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 66.7 / 77.3 | 84.8 / 36.4 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 46.4 / 53.6 | 94.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 98.6 / 100.0 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 74.6 / 83.1 | 85.9 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 100.0 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 84.8 / 72.7 | 93.9 / 97.0 | 48.5 / 3.0 | 3.0 / 0.0 |
| hfc_female ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 22.6 / 23.8 | 98.8 / 100.0 | 51.2 / 25.0 | 42.9 / 61.9 |
| hfc_female ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.8 / 2.8 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| hfc_female ← lessac | 0.0 / 4.2 | 0.0 / 15.1 | 0.0 / 7.1 | 2.9 / 17.1 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 8.5 / 2.8 | 4.2 / 2.8 | 5.7 / 1.9 | 5.7 / 3.8 | 100.0 / 100.0 | 100.0 / 100.0 | 80.3 / 69.0 | 84.5 / 98.6 |
| hfc_female ← libritts_r 120 | 0.0 / 3.0 | 0.0 / 0.0 | 0.0 / 2.5 | 0.0 / 3.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 90.9 / 92.4 | 100.0 / 100.0 | 34.8 / 59.1 | 28.8 / 83.3 |
| hfc_female ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 25.8 / 22.7 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| lessac ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 50.0 / 51.2 | 100.0 / 100.0 | 54.8 / 0.0 | 42.9 / 0.0 |
| lessac ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.8 / 2.8 | 100.0 / 100.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| lessac ← hfc_female | 8.5 / 11.3 | 5.7 / 9.4 | 9.3 / 11.6 | 11.4 / 14.3 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 29.6 / 7.0 | 29.6 / 8.5 | 26.4 / 5.7 | 22.6 / 5.7 | 100.0 / 100.0 | 100.0 / 100.0 | 95.8 / 19.7 | 93.0 / 81.7 |
| lessac ← libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 100.0 | 100.0 / 100.0 | 40.9 / 0.0 | 22.7 / 7.6 |
| lessac ← libritts_r 300 | 0.0 / 1.5 | 0.0 / 2.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 57.6 / 45.5 | 100.0 / 100.0 | 60.6 / 0.0 | 4.5 / 0.0 |
| libritts_r 120 ← amy-low | 31.0 / 4.8 | 41.3 / 4.8 | 48.0 / 10.0 | 47.6 / 9.5 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 1.2 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 81.0 / 66.7 | 96.4 / 77.4 | 98.8 / 100.0 | 98.8 / 100.0 |
| libritts_r 120 ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 25.4 / 7.0 | 73.2 / 29.6 | 5.6 / 0.0 | 1.4 / 0.0 |
| libritts_r 120 ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 94.4 / 78.9 | 93.0 / 95.8 |
| libritts_r 120 ← lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 100.0 | 100.0 / 100.0 | 42.3 / 1.4 | 28.2 / 23.9 |
| libritts_r 120 ← libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 87.9 | 97.0 / 80.3 | 18.2 / 1.5 | 0.0 / 0.0 |
| libritts_r 300 ← amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 71.4 / 56.0 | 85.7 / 0.0 | 4.8 / 0.0 | 0.0 / 0.0 |
| libritts_r 300 ← joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 7.0 / 1.4 | 21.1 / 0.0 | 77.5 / 28.2 | 8.5 / 0.0 |
| libritts_r 300 ← hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 100.0 / 98.6 | 100.0 / 9.9 | 9.9 / 1.4 | 0.0 / 0.0 |
| libritts_r 300 ← lessac | 0.0 / 18.3 | 1.9 / 26.4 | 0.0 / 16.7 | 0.0 / 11.4 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 98.6 / 98.6 | 98.6 / 0.0 | 35.2 / 14.1 | 1.4 / 32.4 |
| libritts_r 300 ← libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 97.0 / 97.0 | 100.0 / 3.0 | 6.1 / 0.0 | 0.0 / 0.0 |

### FRR per caller, bed / clean %

| caller | titanet 1.5s raw @0.459 | titanet 2s raw @0.459 | titanet 2.5s raw @0.459 | titanet 3s raw @0.459 | titanet 2s raw | titanet 2.5s raw | titanet 3s raw | titanet 1.5s s-norm | titanet 1.5s as-norm | titanet 2s s-norm | titanet 2s as-norm | campplus 1.5s raw | campplus 1.5s as-norm | resnet34 1.5s raw | resnet34 1.5s as-norm |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| amy-low | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 14.3 / 2.4 | 0.0 / 3.6 | 0.0 / 0.0 | 0.0 / 0.0 |
| joe | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 1.9 / 1.9 | 4.8 / 2.4 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 12.7 / 1.4 | 14.1 / 0.0 |
| hfc_female | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 5.6 / 0.0 | 7.0 / 0.0 |
| lessac | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 1.9 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| libritts_r 120 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.0 / 0.0 | 2.5 / 0.0 | 0.0 / 0.0 | 1.5 / 0.0 | 21.2 / 4.5 | 0.0 / 0.0 | 36.0 / 10.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 1.5 | 0.0 / 0.0 |
| libritts_r 300 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 2.6 / 0.0 | 3.0 / 0.0 | 18.2 / 19.7 | 15.2 / 22.7 | 18.4 / 30.6 | 14.3 / 30.6 | 1.5 / 4.5 | 0.0 / 98.5 | 0.0 / 0.0 | 3.0 / 0.0 |

### Airport fit (today's gate reproduced offline)

| utterance | fitted start ms | RMSE vs log | reproduced 1.5 s scores |
|---|---|---|---|
| A utt-12 (key 3) | 22500 | 0.051 | -0.034, -0.005, 0.014, -0.058, -0.044, 0.039, 0.155, 0.267, 0.751, 0.706 |
| B utt-10 (key 4) | 36400 | 0.054 | 0.085, -0.024, -0.004, 0.009, 0.589, 0.601, 0.563 |
