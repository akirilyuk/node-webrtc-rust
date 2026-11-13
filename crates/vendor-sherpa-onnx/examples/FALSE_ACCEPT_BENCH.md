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
- **Weaker airport checks and the 2 s AND variants:** for the 2 s candidates and the other models
  `airport pass` reflects the reproduced scores only (no logged scores exist for those windows),
  which is weaker evidence than a 1.5 s pass, since the reproduced raw scores run high. titanet 2s
  raw AND s-norm / raw AND as-norm are not recommended although their FAR is as low: they add
  0.5 s of confirm delay, their airport check cannot be verified at the logged raw scores, and their
  FRR on the libritts_r callers stays high (libritts_r 300 18.4 / 30.6 % with s-norm, 14.3 / 30.6 %
  with as-norm; libritts_r 120 32.0 / 8.0 % with as-norm).

## Recommendation (chunk 1b, Piper voices; superseded by chunk 1c below)

**TitaNet-small, 1.5 s windows, accept only when raw cosine >= 0.459 AND the as-norm score
(top 10 of a fixed 30-speaker cohort) >= 2.672 (tune-matched).** Of the 1.5 s normalized
candidates it is the only one that passes every airport caller window at both the reproduced and
the logged raw scores and does not raise lessac ← hfc_female (as-norm alone also passes the airport
check but raises that pair to 39.4 % bed). It cuts the libritts_r-class false accepts in both directions (amy-low ← libritts_r 120 51.5 → 0 %,
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

## Real speech (chunk 1c, decision j)

Same measurement on LibriSpeech (openslr SLR12, CC BY 4.0), TitaNet-small only. Audio and
embeddings stay in `/tmp` and are never committed. Reproduce:

```text
curl -O https://www.openslr.org/resources/12/test-clean.tar.gz   # and dev-clean.tar.gz
tar xzf test-clean.tar.gz; tar xzf dev-clean.tar.gz
find LibriSpeech -name '*.flac' -print0 | xargs -0 -P 6 -n 100 flac -d -s -f
cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example false_accept_bench -- \
  <same arguments as above> --librispeech <dir>/LibriSpeech --out bench-ls.md
```

Sets: test = all 40 `test-clean` speakers (20 F / 20 M), each one caller and other (1560 pairs).
`dev-clean` has 40 speakers (20 F / 20 M), so "half for tune, 30 for the cohort" cannot both hold:
the 30-speaker cohort size of chunk 1b was kept (15 F / 15 M) and the tune set is the other 10
(5 F / 5 M); all three sets are disjoint. Per speaker 120 s of talk (about 76 windows at 1.5 s),
enrollment 3000 ms from the first chapter, windows from the other chapters. Threshold rule as in
1b (highest threshold with tune FRR <= max(today's tune FRR, 1 %)); today's gate rejects 8.4 % of
the real tune windows, so the target is 8.4 %. Run 2026-11-13, 344 s, heavy-run peak 0.5 GB.

### Findings

- **Today's gate on real speech has a low FAR and a high FRR.** Pooled FAR 0.1 / 0.1 % (bed /
  clean); 29 of 1560 pairs ever false-accept, 9 pairs >= 5 %, 2 pairs >= 20 % (worst 8230 M ←
  1089 M 32.4 / 24.3 %, 1188 M ← 2830 M 30.8 / 2.6 %), almost all same-sex. FRR is 9.3 / 8.1 %
  pooled, 12 of 40 callers above 5 %, and one caller rejects every window (100 %). On real speech
  the gate's weak side is the caller, not the bystander.
- **The Piper `libritts_r` leak does not carry over.** The same gate gives 3.6 % pooled FAR on the
  Piper voices and 0.1 % on real speech; amy-low ← libritts_r 120 (51.5 %) is far above any real
  pair but two.
- **Normalization alone makes real speech worse:** with the real cohort, s-norm 0.2 / 0.3 % and
  as-norm 0.5 / 0.7 % pooled FAR (as-norm: 107 pairs > 0, 32 pairs >= 5 %, clean max 78.7 %),
  and both **fail the airport check** (scene B window 7, margins -0.43 / -0.16). 2 s s-norm fails
  it too; 2 s as-norm passes (reproduced only) but has 0.4 / 0.7 % FAR.
- **Raw AND normalized collapses to today's gate.** With today's tune FRR as the target, the
  normalized part cannot reject any more genuine windows, so the tuned threshold falls below every
  raw-accepted window: raw AND as-norm (2.351) has exactly today's FAR / FRR and worst pairs.
  raw AND s-norm (2.642) also fails airport B window 7 (-0.03). At 2 s the AND variants cut the
  pairs > 0 from 23 to 16 / 19 but keep the worst pair at 44-49 % and add 0.5 s.
- **Longer window (2 s raw, tuned 0.485):** pairs >= 5 % 9 → 5, pooled FAR 0.1 / 0.0 %, but the
  worst pair rises to 39.0 % and FRR stays 9.5 / 7.9 %, for +0.5 s confirm delay.
- **Cross-check, Piper voices with the real cohort and real thresholds:** libritts_r 120 ← amy-low
  still drops, but much less than with the in-family cohort: raw AND as-norm 42.9 → 13.1 % (bed,
  clean 4.8 % unchanged), raw AND s-norm → 8.3 %, s-norm alone → 1.2 %. The decision-g pair
  amy-low ← libritts_r 120 stays at 34.8 / 51.5 % (raw AND as-norm) and 25.8 / 51.5 % (raw AND
  s-norm). So most of chunk 1b's 0 % came from the cohort being the leaking voice's own family.
- **Cross-check, airport at the logged raw scores with the real cohort:** raw AND as-norm passes
  with +0.616 on the normalized part; its binding margin is the raw part, +0.055 (scene B
  window 6, 0.514 vs 0.459), the same as today. s-norm, as-norm and raw AND s-norm fail.
- **Piper cohort on real speech:** s-norm / as-norm alone raise real FRR to 15.9 / 12.9 % and
  reject 85-100 % of the Piper libritts_r callers. raw AND as-norm (Piper cohort, 2.853) keeps
  real FAR at 0.1 / 0.0 % with a lower worst pair (16.7 %), but **fails airport B at the logged
  raw scores** (-0.166). raw AND s-norm (Piper cohort, 2.605) passes the airport by **+0.002**
  and raises libritts_r 300 FRR to 15.2 / 19.7 %. The cohort statistics show the confound
  directly: Piper test windows sit at μt 0.224 against the Piper cohort and 0.119 against the
  real one; real windows sit at 0.104 against the real cohort and 0.116 against the Piper one (bed).

### Recommendation (chunk 1c)

**Keep today's scoring rule (TitaNet-small, 1.5 s, raw cosine >= 0.459) and add no cohort
normalization.** On real speech it has 0.1 % pooled FAR with 9 of 1560 pairs at >= 5 %, it passes
every airport caller window (margin +0.098 reproduced, +0.055 at the logged raw scores), and no
candidate beats it: the normalized scores raise FAR or fail the airport, the AND variants either
equal it (as-norm) or fail the airport (s-norm), and 2 s trades a slightly better pair count for a
worse worst pair and 0.5 s of delay. The decision-g leak (amy-low ← libritts_r 120) is a property
of that Piper voice pair; no rule tested here removes it without failing the airport check or
using an in-family cohort. For chunk 2 this means the gate change in decision g is not supported by
real speech; the parent and user decide whether the split proceeds with today's gate (and a
different bystander voice in the test scenes) or waits for other work. Real-speech FRR (9 %, one
caller at 100 %) is the larger gate problem and is out of this chunk's scope.

**Cohort asset:** if a normalized rule is ever adopted, the cohort must be real speech. A Piper
cohort sits next to the Piper test voices (μt 0.224 vs 0.119), so it suppresses Piper bystanders
by family rather than by speaker, and on real speech it either raises FRR (12.9-15.9 % alone) or
fails the airport check (raw AND as-norm). With the recommendation above, no cohort asset ships.

### Real-speech bench output

Test: LibriSpeech `test-clean`, 40 speakers (20 F / 20 M), each caller and other. Tune: `dev-clean` 10 speakers (84 F, 1462 F, 1673 F, 1919 F, 1988 F, 174 M, 251 M, 422 M, 652 M, 777 M). Cohort: `dev-clean` 30 other speakers (1993 F, 2035 F, 2277 F, 2412 F, 3081 F, 3536 F, 3576 F, 3853 F, 5338 F, 5895 F, 6313 F, 6319 F, 6345 F, 7850 F, 8842 F, 1272 M, 2078 M, 2086 M, 2428 M, 2803 M, 2902 M, 3000 M, 3170 M, 3752 M, 5536 M, 5694 M, 6241 M, 6295 M, 7976 M, 8297 M), disjoint from tune and test. Per speaker: enrollment = 3000 ms from the voiced onset of the first chapter's utterances, talk stream = 120 s from the other chapters (mean 127 s). Windows per speaker and condition: mean 76 (min 60) at 1.5 s, 59 (min 47) at 2.0 s; 1560 (caller, other) pairs. Bed: Tocumen airport at -12 dB (`bed`) or none (`clean`). Thresholds: `@0.459` fixed, else tune-matched: highest threshold with tune FRR <= 8.4 % (today's gate's tune FRR 8.4, floor 1 %). TitaNet embed p50 / p99: 17.6 / 26.4 ms (1.5 s), 21.7 / 29.9 ms (2.0 s).

#### Real speech: summary per candidate

Per-pair FAR distribution over the pairs (bed); FRR distribution over the 40 callers.

| candidate | threshold | test FAR bed / clean % | same-sex FAR bed % | pairs with FAR > 0, bed | pair FAR p50 / p90 / p99 / max bed % | pairs >= 5 % / >= 20 % bed | pair FAR max clean % | test FRR bed / clean % | caller FRR p90 / max bed % | callers FRR > 5 % bed / clean | tune FAR / FRR % | airport min margin reproduced / logged raw | airport pass | extra confirm delay s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1.5s raw @0.459 | 0.459 | 0.1 / 0.1 | 0.2 | 29 / 1560 | 0.0 / 0.0 / 1.6 / 32.4 | 9 / 2 | 24.3 | 9.3 / 8.1 | 22.4 / 100.0 | 12 / 14 | 0.0 / 8.4 | +0.098 / +0.055 | yes | 0.0 |
| 2s raw @0.459 | 0.459 | 0.1 / 0.1 | 0.3 | 23 / 1560 | 0.0 / 0.0 / 1.8 / 55.9 | 10 / 3 | 33.9 | 7.2 / 5.8 | 19.4 / 100.0 | 11 / 9 | 0.0 / 5.0 | +0.134 / - | yes | 0.5 |
| 1.5s raw | 0.459 | 0.1 / 0.1 | 0.2 | 29 / 1560 | 0.0 / 0.0 / 1.6 / 32.4 | 9 / 2 | 24.3 | 9.3 / 8.1 | 22.4 / 100.0 | 12 / 14 | 0.0 / 8.4 | +0.097 / +0.055 | yes | 0.0 |
| 2s raw | 0.485 | 0.1 / 0.0 | 0.1 | 13 / 1560 | 0.0 / 0.0 / 0.0 / 39.0 | 5 / 2 | 13.6 | 9.5 / 7.9 | 25.4 / 100.0 | 14 / 11 | 0.0 / 8.3 | +0.107 / - | yes | 0.5 |
| 1.5s s-norm | 3.031 | 0.2 / 0.3 | 0.4 | 53 / 1560 | 0.0 / 0.0 / 3.9 / 47.4 | 14 / 6 | 58.1 | 7.1 / 5.0 | 10.4 / 100.0 | 8 / 3 | 0.0 / 8.4 | -0.423 / -0.431 | **no** | 0.0 |
| 1.5s as-norm | 3.142 | 0.5 / 0.7 | 0.9 | 107 / 1560 | 0.0 / 0.0 / 12.5 / 51.8 | 32 / 14 | 78.7 | 7.0 / 5.1 | 9.0 / 100.0 | 6 / 3 | 0.1 / 8.4 | -0.164 / -0.176 | **no** | 0.0 |
| 1.5s raw AND s-norm | 2.642 | 0.1 / 0.1 | 0.2 | 29 / 1560 | 0.0 / 0.0 / 1.6 / 32.4 | 9 / 2 | 24.3 | 9.6 / 8.1 | 23.5 / 100.0 | 12 / 14 | 0.0 / 8.4 | -0.034 / -0.042 | **no** | 0.0 |
| 1.5s raw AND as-norm | 2.351 | 0.1 / 0.1 | 0.2 | 29 / 1560 | 0.0 / 0.0 / 1.6 / 32.4 | 9 / 2 | 24.3 | 9.8 / 8.2 | 23.5 / 100.0 | 12 / 14 | 0.0 / 8.4 | +0.628 / +0.616 | yes | 0.0 |
| 2s s-norm | 3.165 | 0.2 / 0.2 | 0.3 | 30 / 1560 | 0.0 / 0.0 / 2.1 / 48.3 | 8 / 3 | 62.7 | 7.1 / 4.5 | 9.1 / 100.0 | 9 / 4 | 0.0 / 8.3 | -0.075 / - | **no** | 0.5 |
| 2s as-norm | 3.284 | 0.4 / 0.7 | 0.8 | 69 / 1560 | 0.0 / 0.0 / 10.0 / 55.0 | 28 / 10 | 81.7 | 6.8 / 4.7 | 5.7 / 100.0 | 8 / 3 | 0.1 / 8.3 | +0.594 / - | yes | 0.5 |
| 2s raw AND s-norm | 3.126 | 0.1 / 0.1 | 0.2 | 16 / 1560 | 0.0 / 0.0 / 0.0 / 44.1 | 7 / 3 | 33.9 | 8.6 / 6.3 | 23.6 / 100.0 | 12 / 9 | 0.0 / 8.3 | -0.037 / - | **no** | 0.5 |
| 2s raw AND as-norm | 3.202 | 0.1 / 0.1 | 0.3 | 19 / 1560 | 0.0 / 0.0 / 1.7 / 49.2 | 10 / 3 | 33.9 | 9.1 / 6.8 | 23.6 / 100.0 | 12 / 9 | 0.0 / 8.3 | +0.676 / - | yes | 0.5 |
| 1.5s s-norm (Piper cohort) | 3.791 | 0.3 / 0.7 | 0.5 | 51 / 1560 | 0.0 / 0.0 / 7.9 / 37.6 | 24 / 7 | 92.0 | 15.9 / 7.6 | 41.2 / 97.4 | 17 / 10 | 0.2 / 8.4 | -0.797 / -1.184 | **no** | 0.0 |
| 1.5s as-norm (Piper cohort) | 4.380 | 0.5 / 1.0 | 0.8 | 96 / 1560 | 0.0 / 0.0 / 13.7 / 77.9 | 39 / 10 | 92.2 | 12.9 / 6.6 | 40.0 / 92.1 | 14 / 8 | 0.3 / 8.4 | -1.242 / -1.692 | **no** | 0.0 |
| 1.5s raw AND s-norm (Piper cohort) | 2.605 | 0.1 / 0.0 | 0.2 | 26 / 1560 | 0.0 / 0.0 / 1.4 / 21.8 | 8 / 1 | 10.3 | 9.5 / 8.1 | 23.5 / 100.0 | 13 / 14 | 0.0 / 8.4 | +0.389 / +0.002 | yes | 0.0 |
| 1.5s raw AND as-norm (Piper cohort) | 2.853 | 0.1 / 0.0 | 0.1 | 26 / 1560 | 0.0 / 0.0 / 1.4 / 16.7 | 6 / 0 | 10.3 | 9.7 / 8.2 | 23.5 / 100.0 | 14 / 15 | 0.0 / 8.4 | +0.284 / -0.166 | **no** | 0.0 |

#### Real speech: worst 10 pairs per candidate (caller ← other, bed / clean %)

| candidate | worst pairs |
|---|---|
| 1.5s raw @0.459 | 8230 M ← 1089 M 32.4 / 24.3; 1188 M ← 2830 M 30.8 / 2.6; 5142 F ← 4970 F 16.7 / 8.3; 8230 M ← 61 M 15.5 / 11.9; 7729 M ← 908 M 10.8 / 3.6; 1089 M ← 8230 M 10.7 / 8.0; 61 M ← 8230 M 6.7 / 1.3; 7021 M ← 1188 M 6.2 / 0.0; 260 M ← 1284 F 5.5 / 0.0; 3729 F ← 4507 F 4.0 / 2.7 |
| 2s raw @0.459 | 8230 M ← 1089 M 55.9 / 33.9; 1188 M ← 2830 M 43.3 / 6.7; 8230 M ← 61 M 25.4 / 19.0; 5142 F ← 4970 F 19.1 / 10.6; 7729 M ← 908 M 9.5 / 6.3; 1089 M ← 8230 M 8.3 / 5.0; 61 M ← 8230 M 6.7 / 3.3; 4507 F ← 3729 F 6.1 / 0.0; 3729 F ← 4507 F 5.0 / 1.7; 7021 M ← 1188 M 5.0 / 0.0 |
| 1.5s raw | 8230 M ← 1089 M 32.4 / 24.3; 1188 M ← 2830 M 30.8 / 2.6; 5142 F ← 4970 F 16.7 / 8.3; 8230 M ← 61 M 15.5 / 11.9; 7729 M ← 908 M 10.8 / 3.6; 1089 M ← 8230 M 10.7 / 8.0; 61 M ← 8230 M 6.7 / 1.3; 7021 M ← 1188 M 6.2 / 0.0; 260 M ← 1284 F 5.5 / 0.0; 3729 F ← 4507 F 4.0 / 2.7 |
| 2s raw | 8230 M ← 1089 M 39.0 / 13.6; 1188 M ← 2830 M 23.3 / 1.7; 8230 M ← 61 M 14.3 / 9.5; 5142 F ← 4970 F 12.8 / 4.3; 7729 M ← 908 M 6.3 / 4.8; 7021 M ← 1320 M 4.8 / 1.6; 1089 M ← 8230 M 3.3 / 0.0; 7021 M ← 8455 M 2.0 / 0.0; 61 M ← 8230 M 1.7 / 1.7; 3729 F ← 4507 F 1.7 / 1.7 |
| 1.5s s-norm | 1188 M ← 2830 M 47.4 / 23.1; 8230 M ← 1089 M 47.3 / 58.1; 7729 M ← 908 M 36.1 / 45.8; 8230 M ← 61 M 21.4 / 34.5; 1089 M ← 8230 M 21.3 / 32.0; 8463 F ← 7729 M 20.8 / 41.6; 61 M ← 8230 M 16.0 / 18.7; 2094 F ← 1995 F 10.1 / 3.8; 5142 F ← 4970 F 10.0 / 28.3; 8555 F ← 5142 F 8.6 / 4.9 |
| 1.5s as-norm | 7729 M ← 908 M 51.8 / 60.2; 1188 M ← 2830 M 47.4 / 42.3; 8230 M ← 1089 M 44.6 / 67.6; 2094 F ← 1995 F 44.3 / 25.3; 61 M ← 8230 M 28.0 / 34.7; 5142 F ← 4970 F 26.7 / 43.3; 1089 M ← 8230 M 25.3 / 78.7; 7021 M ← 1188 M 25.0 / 47.5; 8230 M ← 61 M 25.0 / 44.0; 8463 F ← 7729 M 23.4 / 57.1 |
| 1.5s raw AND s-norm | 8230 M ← 1089 M 32.4 / 24.3; 1188 M ← 2830 M 30.8 / 2.6; 5142 F ← 4970 F 16.7 / 8.3; 8230 M ← 61 M 15.5 / 11.9; 7729 M ← 908 M 10.8 / 3.6; 1089 M ← 8230 M 10.7 / 8.0; 61 M ← 8230 M 6.7 / 1.3; 7021 M ← 1188 M 6.2 / 0.0; 260 M ← 1284 F 5.5 / 0.0; 3729 F ← 4507 F 4.0 / 2.7 |
| 1.5s raw AND as-norm | 8230 M ← 1089 M 32.4 / 24.3; 1188 M ← 2830 M 30.8 / 2.6; 5142 F ← 4970 F 16.7 / 8.3; 8230 M ← 61 M 15.5 / 11.9; 7729 M ← 908 M 10.8 / 3.6; 1089 M ← 8230 M 10.7 / 8.0; 61 M ← 8230 M 6.7 / 1.3; 7021 M ← 1188 M 6.2 / 0.0; 260 M ← 1284 F 5.5 / 0.0; 3729 F ← 4507 F 4.0 / 2.7 |
| 2s s-norm | 1188 M ← 2830 M 48.3 / 20.0; 8230 M ← 1089 M 44.1 / 62.7; 7729 M ← 908 M 31.7 / 36.5; 8230 M ← 61 M 19.0 / 25.4; 1089 M ← 8230 M 15.0 / 31.7; 61 M ← 8230 M 11.7 / 13.3; 8463 F ← 7729 M 10.0 / 28.3; 5142 F ← 4970 F 6.4 / 34.0; 8555 F ← 5142 F 4.9 / 3.3; 4507 F ← 3729 F 4.5 / 4.5 |
| 2s as-norm | 1188 M ← 2830 M 55.0 / 48.3; 8230 M ← 1089 M 49.2 / 76.3; 7729 M ← 908 M 47.6 / 65.1; 2094 F ← 1995 F 38.3 / 23.3; 5142 F ← 4970 F 34.0 / 53.2; 8230 M ← 61 M 31.7 / 38.1; 61 M ← 8230 M 31.7 / 35.0; 1089 M ← 8230 M 26.7 / 81.7; 260 M ← 8224 M 23.8 / 1.6; 4507 F ← 3729 F 21.2 / 10.6 |
| 2s raw AND s-norm | 8230 M ← 1089 M 44.1 / 33.9; 1188 M ← 2830 M 38.3 / 6.7; 8230 M ← 61 M 22.2 / 19.0; 7729 M ← 908 M 9.5 / 6.3; 61 M ← 8230 M 6.7 / 3.3; 1089 M ← 8230 M 6.7 / 5.0; 5142 F ← 4970 F 6.4 / 8.5; 4507 F ← 3729 F 4.5 / 0.0; 7021 M ← 8455 M 3.9 / 0.0; 1188 M ← 1089 M 1.7 / 1.7 |
| 2s raw AND as-norm | 8230 M ← 1089 M 49.2 / 33.9; 1188 M ← 2830 M 38.3 / 6.7; 8230 M ← 61 M 23.8 / 19.0; 5142 F ← 4970 F 19.1 / 8.5; 7729 M ← 908 M 9.5 / 6.3; 61 M ← 8230 M 6.7 / 3.3; 1089 M ← 8230 M 6.7 / 5.0; 4507 F ← 3729 F 6.1 / 0.0; 3729 F ← 4507 F 5.0 / 1.7; 7021 M ← 1188 M 5.0 / 0.0 |
| 1.5s s-norm (Piper cohort) | 4992 F ← 8463 F 37.6 / 80.0; 237 F ← 5142 F 35.8 / 37.0; 8463 F ← 7729 M 35.1 / 53.2; 3729 F ← 4507 F 28.0 / 92.0; 8463 F ← 4992 F 25.4 / 53.7; 4992 F ← 4507 F 20.0 / 40.0; 5142 F ← 4970 F 20.0 / 40.0; 8455 M ← 7127 M 16.9 / 38.5; 4507 F ← 3729 F 14.8 / 18.2; 5105 M ← 4446 F 14.5 / 42.1 |
| 1.5s as-norm (Piper cohort) | 8463 F ← 7729 M 77.9 / 92.2; 8463 F ← 4992 F 46.3 / 83.6; 4992 F ← 8463 F 42.4 / 74.1; 237 F ← 5142 F 39.5 / 42.0; 3729 F ← 4507 F 37.3 / 78.7; 5105 M ← 4446 F 28.9 / 40.8; 8463 F ← 4507 F 28.0 / 25.3; 4507 F ← 3729 F 26.1 / 26.1; 5142 F ← 4970 F 25.0 / 43.3; 237 F ← 4970 F 20.0 / 18.3 |
| 1.5s raw AND s-norm (Piper cohort) | 1188 M ← 2830 M 21.8 / 2.6; 5142 F ← 4970 F 16.7 / 8.3; 8230 M ← 1089 M 13.5 / 9.5; 7729 M ← 908 M 10.8 / 3.6; 1089 M ← 8230 M 9.3 / 8.0; 7021 M ← 1188 M 6.2 / 0.0; 260 M ← 1284 F 5.5 / 0.0; 61 M ← 8230 M 5.3 / 1.3; 3729 F ← 4507 F 4.0 / 2.7; 672 M ← 1320 M 3.8 / 10.3 |
| 1.5s raw AND as-norm (Piper cohort) | 5142 F ← 4970 F 16.7 / 8.3; 7729 M ← 908 M 10.8 / 3.6; 1089 M ← 8230 M 10.7 / 8.0; 1188 M ← 2830 M 6.4 / 2.6; 7021 M ← 1188 M 6.2 / 0.0; 260 M ← 1284 F 5.5 / 0.0; 8230 M ← 1089 M 4.1 / 1.4; 3729 F ← 4507 F 4.0 / 2.7; 672 M ← 1320 M 3.8 / 10.3; 260 M ← 8224 M 3.6 / 0.0 |

#### Cross-check: Piper test voices scored with the real-speech thresholds and cohort

Same windows as chunk 1b (bed / clean %).

| candidate | threshold | Piper FAR bed / clean | amy-low ← libritts_r 120 | libritts_r 120 ← amy-low | libritts_r 300 ← lessac | lessac ← hfc_female | worst pair bed | FRR libritts_r 120 | FRR libritts_r 300 | max FRR other callers bed / clean |
|---|---|---|---|---|---|---|---|---|---|---|
| 1.5s raw @0.459 | 0.459 | 3.6 / 3.2 | 51.5 / 51.5 | 42.9 / 4.8 | 0.0 / 18.3 | 9.9 / 15.5 | 51.5 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 2s raw @0.459 | 0.459 | 4.2 / 4.1 | 58.0 / 58.0 | 52.4 / 4.8 | 1.9 / 26.4 | 9.4 / 15.1 | 58.0 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 1.5s raw | 0.459 | 3.6 / 3.2 | 51.5 / 51.5 | 42.9 / 4.8 | 0.0 / 18.3 | 9.9 / 15.5 | 51.5 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 2s raw | 0.485 | 2.9 / 1.8 | 44.0 / 36.0 | 34.9 / 1.6 | 0.0 / 5.7 | 3.8 / 7.5 | 44.0 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 1.5s s-norm | 3.031 | 0.9 / 7.6 | 10.6 / 39.4 | 1.2 / 1.2 | 1.4 / 57.7 | 11.3 / 74.6 | 11.3 (lessac ← hfc_female) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 1.5s as-norm | 3.142 | 2.6 / 11.5 | 9.1 / 42.4 | 3.6 / 7.1 | 4.2 / 59.2 | 36.6 / 94.4 | 36.6 (lessac ← hfc_female) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 1.5s raw AND s-norm | 2.642 | 1.5 / 3.2 | 25.8 / 51.5 | 8.3 / 4.8 | 0.0 / 18.3 | 9.9 / 15.5 | 25.8 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 1.5s raw AND as-norm | 2.351 | 2.0 / 3.2 | 34.8 / 51.5 | 13.1 / 4.8 | 0.0 / 18.3 | 9.9 / 15.5 | 34.8 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 2s s-norm | 3.165 | 0.4 / 6.5 | 8.0 / 34.0 | 0.0 / 0.0 | 1.9 / 45.3 | 1.9 / 67.9 | 8.0 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 2s as-norm | 3.284 | 2.8 / 11.1 | 6.0 / 42.0 | 0.0 / 4.8 | 7.5 / 58.5 | 43.4 / 98.1 | 43.4 (lessac ← hfc_female) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 2s raw AND s-norm | 3.126 | 0.4 / 3.3 | 8.0 / 36.0 | 0.0 / 1.6 | 1.9 / 26.4 | 3.8 / 15.1 | 8.0 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 2s raw AND as-norm | 3.202 | 0.8 / 3.5 | 14.0 / 42.0 | 0.0 / 1.6 | 1.9 / 26.4 | 9.4 / 15.1 | 14.0 (amy-low ← libritts_r 120) | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 |
| 1.5s s-norm (Piper cohort) | 3.791 | 0.3 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 8.5 / 0.0 | 8.5 (lessac ← hfc_female) | 84.8 / 34.8 | 100.0 / 100.0 | 6.0 / 0.0 |
| 1.5s as-norm (Piper cohort) | 4.380 | 0.4 / 0.1 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 12.7 / 2.8 | 12.7 (lessac ← hfc_female) | 95.5 / 92.4 | 92.4 / 100.0 | 15.5 / 0.0 |
| 1.5s raw AND s-norm (Piper cohort) | 2.605 | 0.4 / 0.4 | 0.0 / 0.0 | 1.2 / 0.0 | 0.0 / 0.0 | 9.9 / 8.5 | 9.9 (lessac ← hfc_female) | 0.0 / 0.0 | 15.2 / 19.7 | 0.0 / 0.0 |
| 1.5s raw AND as-norm (Piper cohort) | 2.853 | 0.3 / 0.3 | 0.0 / 0.0 | 0.0 / 0.0 | 0.0 / 0.0 | 9.9 / 8.5 | 9.9 (lessac ← hfc_female) | 28.8 / 6.1 | 27.3 / 30.3 | 0.0 / 0.0 |

#### Cross-check: airport demo caller windows

Reproduced offline (enrollment and window embeddings) and at the logged raw cosine (1.5 s only; cohort statistics from the reproduced embeddings). Cohort as in the candidate; bed condition cohort.

| candidate | threshold | A w9, w10 reproduced | B w5, w6, w7 reproduced | A / B at logged raw | min margin reproduced / logged |
|---|---|---|---|---|---|
| 1.5s raw @0.459 | 0.459 | 0.744, 0.714 | 0.587, 0.606, 0.557 | 0.697, 0.650 / 0.515, 0.514, 0.562 | +0.098 / +0.055 |
| 2s raw @0.459 | 0.459 | 0.742, 0.770 | 0.593, 0.653, 0.618 | - | +0.134 / - |
| 1.5s raw | 0.459 | 0.744, 0.714 | 0.587, 0.606, 0.557 | 0.697, 0.650 / 0.515, 0.514, 0.562 | +0.097 / +0.055 |
| 2s raw | 0.485 | 0.742, 0.770 | 0.593, 0.653, 0.618 | - | +0.107 / - |
| 1.5s s-norm | 3.031 | 4.155, 4.356 | 3.132, 3.284, 2.608 | 3.827, 3.868 / 2.600, 2.605, 2.646 | -0.423 / -0.431 |
| 1.5s as-norm | 3.142 | 5.250, 5.934 | 4.137, 4.737, 2.978 | 4.663, 5.015 / 2.967, 3.155, 3.062 | -0.164 / -0.176 |
| 1.5s raw AND s-norm | 2.642 | 4.155, 4.356 | 3.132, 3.284, 2.608 | 3.827, 3.868 / 2.600, 2.605, 2.646 | -0.034 / -0.042 |
| 1.5s raw AND as-norm | 2.351 | 5.250, 5.934 | 4.137, 4.737, 2.978 | 4.663, 5.015 / 2.967, 3.155, 3.062 | +0.628 / +0.616 |
| 2s s-norm | 3.165 | 4.026, 4.580 | 3.090, 3.497, 3.113 | - | -0.075 / - |
| 2s as-norm | 3.284 | 5.082, 6.249 | 4.348, 5.115, 3.878 | - | +0.594 / - |
| 2s raw AND s-norm | 3.126 | 4.026, 4.580 | 3.090, 3.497, 3.113 | - | -0.037 / - |
| 2s raw AND as-norm | 3.202 | 5.082, 6.249 | 4.348, 5.115, 3.878 | - | +0.676 / - |
| 1.5s s-norm (Piper cohort) | 3.791 | 5.278, 4.868 | 3.187, 3.538, 2.993 | 4.857, 4.308 / 2.606, 2.749, 3.038 | -0.797 / -1.184 |
| 1.5s as-norm (Piper cohort) | 4.380 | 6.847, 5.795 | 3.879, 4.056, 3.137 | 6.131, 4.905 / 2.707, 2.687, 3.215 | -1.242 / -1.692 |
| 1.5s raw AND s-norm (Piper cohort) | 2.605 | 5.278, 4.868 | 3.187, 3.538, 2.993 | 4.857, 4.308 / 2.606, 2.749, 3.038 | +0.389 / +0.002 |
| 1.5s raw AND as-norm (Piper cohort) | 2.853 | 6.847, 5.795 | 3.879, 4.056, 3.137 | 6.131, 4.905 / 2.707, 2.687, 3.215 | +0.284 / -0.166 |

#### Cohort statistics (TitaNet 1.5 s windows, bed / clean)

Mean over all windows of the set of the window-side cohort mean / std (`μt` / `σt`), full cohort.

| set | real-speech cohort μt / σt | Piper cohort μt / σt |
|---|---|---|
| LibriSpeech test-clean | 0.104 / 0.104 ; 0.079 / 0.104 | 0.116 / 0.090 ; 0.079 / 0.085 |
| Piper test voices | 0.119 / 0.114 ; 0.071 / 0.109 | 0.224 / 0.101 ; 0.249 / 0.093 |

## Bench output (chunk 1b, Piper voices)

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
