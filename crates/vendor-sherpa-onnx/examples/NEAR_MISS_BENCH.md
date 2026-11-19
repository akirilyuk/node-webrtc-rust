# Near-miss re-check and other-to-caller cut bench

`near_miss_bench.rs` measures two things for the speaker gate's
`splitOnSpeakerChange` path (`crates/speech/src/speaker_gate.rs`):

1. **Near-miss re-check.** A gate window on an `other` utterance that scores in
   `[0.30, threshold)` is re-scanned with short sub-windows. One caller
   sub-window confirms the window as the caller.
2. **Other-to-caller cut.** Where the first caller utterance (U2) starts after a
   confirmed other-to-caller change.

No audio or embeddings are committed. The bench builds scenes at run time from
local corpora and models.

## Sources

- Speaker embedder: NeMo TitaNet-small (`nemo_en_titanet_small.onnx`), threshold 0.459.
- Recognizer: sherpa-onnx 1.13.2 streaming zipformer
  `sherpa-onnx-streaming-zipformer-en-2023-06-26`, greedy search. Words are
  taken from the documented `RecognizerResult` fields `tokens` and `timestamps`.
- Speech: LibriSpeech (OpenSLR 12) `test-clean` for tuning, `dev-clean` for the
  held-out check; Piper TTS (`vits-piper-en_US-amy-low`) fixtures, with a fresh
  reroll for the held-out check.
- Background bed: Tocumen airport recording at -12 dB for the "bed" sets.

## Method A (primary bar): recognizer diff

- Reference: recognizer words on the caller piece alone.
- U2 head: recognizer words on `[cut, max(cut, onset) + 2500 ms)`, bed included.
- Lost words: reference words deleted at the head before the first exact match
  (word edit-distance alignment), minus the floor (the same count with the cut
  at the true caller onset). A scene with no confirm loses all its floor words.
- Leaked words: head-inserted words that occur in the bystander's own decode and
  not in the caller reference.
- Method B (cross-check only): word times from relative recognizer timestamps.
  The recognizer emits timestamps about 400 ms late, so B is not the bar.

## Configs

- today: stage 1 (longest quiet run in `[prev.start, span.start + 300]`, used
  when it is at least 500 ms), else stage 2 (1250/250 sub-windows from the
  previous window's start, then a forward quiet run within 500 ms).
- **A (picked)**: no stage 1; 750/125 sub-windows from the previous window's
  start; cut = first caller sub-window start minus 300 ms, clamped to the
  previous window's start; no caller sub-window or timeout: the previous
  window's start.
- B: as A with a 200 ms back-off. A' / B': stage 1 kept, clamped to A / B.

## Run

Usage is in the module doc at the top of `near_miss_bench.rs` (`--model`,
`--fixtures`, `--straddle-piper`, `--librispeech`, `--ls-split`, `--bed`, `--stt`).

`NM_CFGS=x` selects the five configs above, `NM_SWEEP_TSV=<path>` writes the
per-scene sweep, `NM_SPLIT_DUMP=<dir>` writes the split inputs read by the
ignored `bench_split_parity` test in `speaker_gate.rs`.

## Tuning sweep (LibriSpeech test-clean, Piper fixtures)

## Step 1: attribution of today's losses (today-confirmed scenes; path 0 stage 1, 1 stage 2 caller sub-window, 2 stage 2 none -> fallback)

| set      | path                           | cut vs onset    | scenes | caller words lost above floor |
| -------- | ------------------------------ | --------------- | ------ | ----------------------------- |
| Piper    | stage 1 quiet run              | after onset     | 16     | 10                            |
| Piper    | stage 1 quiet run              | at/before onset | 32     | 0                             |
| Piper    | stage 2, caller sub-window     | after onset     | 53     | 14                            |
| Piper    | stage 2, caller sub-window     | at/before onset | 290    | 7                             |
| Piper    | stage 2, none -> stage-1 point | at/before onset | 6      | 0                             |
| LS-bed   | stage 1 quiet run              | after onset     | 8      | 5                             |
| LS-bed   | stage 1 quiet run              | at/before onset | 8      | 0                             |
| LS-bed   | stage 2, caller sub-window     | after onset     | 18     | 6                             |
| LS-bed   | stage 2, caller sub-window     | at/before onset | 33     | 0                             |
| LS-clean | stage 1 quiet run              | after onset     | 12     | 4                             |
| LS-clean | stage 1 quiet run              | at/before onset | 12     | 0                             |
| LS-clean | stage 2, caller sub-window     | after onset     | 32     | 4                             |
| LS-clean | stage 2, caller sub-window     | at/before onset | 44     | 0                             |

Piper: scenes 398, confirmed today 397, floor words 1502

LS-bed: scenes 89, confirmed today 67, floor words 343

LS-clean: scenes 117, confirmed today 100, floor words 456

## Step 2: cut settings on today-confirmed scenes (lost = caller words lost above the floor; leak = bystander words in the U2 head)

| rank  | config                                                     | LS-bed lost | LS-bed leak | LS-clean lost | LS-clean leak | Piper lost | Piper leak | paths S1/S2/FB (LS-clean) |
| ----- | ---------------------------------------------------------- | ----------- | ----------- | ------------- | ------------- | ---------- | ---------- | ------------------------- |
| today | s2=fwd500 sub=1250/250 s1min=500 s1search=+300 fb=stage1   | 11          | 7           | 8             | 7             | 31         | 41         | 24/76/0                   |
| 1     | s2=p-300 sub=750/125 s1min=off s1search=+0 fb=prevstart    | 1           | 21          | 1             | 18            | 0          | 83         | 0/98/2                    |
| 2     | s2=p-300 sub=750/125 s1min=off s1search=+300 fb=prevstart  | 1           | 21          | 1             | 18            | 0          | 83         | 0/98/2                    |
| 3     | s2=p-200 sub=1000/125 s1min=off s1search=+0 fb=stage1      | 2           | 25          | 0             | 28            | 1          | 132        | 0/100/0                   |
| 4     | s2=p-200 sub=1000/125 s1min=off s1search=+300 fb=stage1    | 2           | 25          | 0             | 28            | 1          | 137        | 0/100/0                   |
| 5     | s2=p-200 sub=1000/125 s1min=off s1search=+0 fb=prevstart   | 2           | 25          | 0             | 28            | 1          | 150        | 0/100/0                   |
| 6     | s2=p-200 sub=1000/125 s1min=off s1search=+300 fb=prevstart | 2           | 25          | 0             | 28            | 1          | 150        | 0/100/0                   |
| 7     | s2=p-200 sub=750/125 s1min=off s1search=+0 fb=prevstart    | 0           | 14          | 3             | 16            | 0          | 44         | 0/98/2                    |
| 8     | s2=p-200 sub=750/125 s1min=off s1search=+300 fb=prevstart  | 0           | 14          | 3             | 16            | 0          | 44         | 0/98/2                    |
| 9     | s2=p-200 sub=750/125 s1min=800 s1search=+0 fb=prevstart    | 1           | 10          | 2             | 14            | 2          | 44         | 2/97/1                    |
| 10    | s2=p-200 sub=750/125 s1min=800 s1search=+300 fb=prevstart  | 1           | 10          | 2             | 14            | 2          | 44         | 4/95/1                    |
| 11    | s2=p-300 sub=750/125 s1min=800 s1search=+300 fb=prevstart  | 2           | 17          | 1             | 16            | 2          | 82         | 4/95/1                    |
| 12    | s2=p-300 sub=750/125 s1min=800 s1search=+0 fb=prevstart    | 2           | 17          | 1             | 16            | 2          | 83         | 2/97/1                    |
| 13    | s2=p-200 sub=1000/125 s1min=800 s1search=+0 fb=stage1      | 3           | 24          | 0             | 27            | 3          | 129        | 2/98/0                    |
| 14    | s2=p-200 sub=1000/125 s1min=800 s1search=+300 fb=stage1    | 3           | 24          | 0             | 27            | 3          | 133        | 4/96/0                    |
| 15    | s2=p-200 sub=1000/125 s1min=800 s1search=+300 fb=prevstart | 3           | 24          | 0             | 27            | 3          | 146        | 4/96/0                    |
| 16    | s2=p-200 sub=1000/125 s1min=800 s1search=+0 fb=prevstart   | 3           | 24          | 0             | 27            | 3          | 147        | 2/98/0                    |
| 17    | s2=p-0 sub=1000/125 s1min=off s1search=+0 fb=stage1        | 3           | 7           | 2             | 9             | 0          | 52         | 0/100/0                   |

## Held-out validation (LibriSpeech dev-clean, rerolled Piper)

| set                                      | gate                    | cut                       | scenes | floor words | confirmed | words lost (all) | lost on today-confirmed | recovered scenes / words | bystander words leaked | paths S1/S2/FB |
| ---------------------------------------- | ----------------------- | ------------------------- | ------ | ----------- | --------- | ---------------- | ----------------------- | ------------------------ | ---------------------- | -------------- |
| Piper fixtures, merged no-pause          | today (no re-check)     | today                     | 250    | 2881        | 249       | 25               | 21                      | 2 / 31                   | 24                     | 38/209/2       |
| Piper fixtures, merged no-pause          | today (no re-check)     | A (p-300)                 | 250    | 2881        | 249       | 25               | 21                      | 2 / 31                   | 44                     | 0/245/4        |
| Piper fixtures, merged no-pause          | today (no re-check)     | B (p-200)                 | 250    | 2881        | 249       | 31               | 27                      | 2 / 31                   | 29                     | 0/245/4        |
| Piper fixtures, merged no-pause          | today (no re-check)     | A' (stage 1 clamped to A) | 250    | 2881        | 249       | 24               | 20                      | 2 / 31                   | 45                     | 38/207/4       |
| Piper fixtures, merged no-pause          | today (no re-check)     | B' (stage 1 clamped to B) | 250    | 2881        | 249       | 29               | 25                      | 2 / 31                   | 30                     | 38/207/4       |
| Piper fixtures, merged no-pause          | band [0.30, thr) need 1 | today                     | 250    | 2881        | 250       | 19               | 19                      | 3 / 35                   | 24                     | 31/217/2       |
| Piper fixtures, merged no-pause          | band [0.30, thr) need 1 | A (p-300)                 | 250    | 2881        | 250       | 20               | 20                      | 3 / 35                   | 63                     | 0/246/4        |
| Piper fixtures, merged no-pause          | band [0.30, thr) need 1 | B (p-200)                 | 250    | 2881        | 250       | 26               | 26                      | 3 / 35                   | 36                     | 0/246/4        |
| Piper fixtures, merged no-pause          | band [0.30, thr) need 1 | A' (stage 1 clamped to A) | 250    | 2881        | 250       | 19               | 19                      | 3 / 35                   | 64                     | 31/215/4       |
| Piper fixtures, merged no-pause          | band [0.30, thr) need 1 | B' (stage 1 clamped to B) | 250    | 2881        | 250       | 24               | 24                      | 3 / 35                   | 37                     | 31/215/4       |
| Piper straddle                           | today (no re-check)     | today                     | 355    | 1298        | 351       | 50               | 35                      | 0 / 0                    | 15                     | 35/312/4       |
| Piper straddle                           | today (no re-check)     | A (p-300)                 | 355    | 1298        | 351       | 15               | 0                       | 0 / 0                    | 93                     | 0/345/6        |
| Piper straddle                           | today (no re-check)     | B (p-200)                 | 355    | 1298        | 351       | 15               | 0                       | 0 / 0                    | 50                     | 0/345/6        |
| Piper straddle                           | today (no re-check)     | A' (stage 1 clamped to A) | 355    | 1298        | 351       | 15               | 0                       | 0 / 0                    | 94                     | 35/310/6       |
| Piper straddle                           | today (no re-check)     | B' (stage 1 clamped to B) | 355    | 1298        | 351       | 15               | 0                       | 0 / 0                    | 53                     | 35/310/6       |
| Piper straddle                           | band [0.30, thr) need 1 | today                     | 355    | 1298        | 355       | 31               | 31                      | 4 / 15                   | 15                     | 29/322/4       |
| Piper straddle                           | band [0.30, thr) need 1 | A (p-300)                 | 355    | 1298        | 355       | 0                | 0                       | 4 / 15                   | 141                    | 0/349/6        |
| Piper straddle                           | band [0.30, thr) need 1 | B (p-200)                 | 355    | 1298        | 355       | 0                | 0                       | 4 / 15                   | 83                     | 0/349/6        |
| Piper straddle                           | band [0.30, thr) need 1 | A' (stage 1 clamped to A) | 355    | 1298        | 355       | 0                | 0                       | 4 / 15                   | 142                    | 29/320/6       |
| Piper straddle                           | band [0.30, thr) need 1 | B' (stage 1 clamped to B) | 355    | 1298        | 355       | 0                | 0                       | 4 / 15                   | 86                     | 29/320/6       |
| LibriSpeech straddle, Tocumen bed -12 dB | today (no re-check)     | today                     | 80     | 335         | 62        | 76               | 12                      | 0 / 0                    | 8                      | 17/45/0        |
| LibriSpeech straddle, Tocumen bed -12 dB | today (no re-check)     | A (p-300)                 | 80     | 335         | 62        | 65               | 1                       | 0 / 0                    | 13                     | 0/59/3         |
| LibriSpeech straddle, Tocumen bed -12 dB | today (no re-check)     | B (p-200)                 | 80     | 335         | 62        | 69               | 5                       | 0 / 0                    | 8                      | 0/59/3         |
| LibriSpeech straddle, Tocumen bed -12 dB | today (no re-check)     | A' (stage 1 clamped to A) | 80     | 335         | 62        | 65               | 1                       | 0 / 0                    | 20                     | 17/42/3        |
| LibriSpeech straddle, Tocumen bed -12 dB | today (no re-check)     | B' (stage 1 clamped to B) | 80     | 335         | 62        | 68               | 4                       | 0 / 0                    | 15                     | 17/42/3        |
| LibriSpeech straddle, Tocumen bed -12 dB | band [0.30, thr) need 1 | today                     | 80     | 335         | 68        | 58               | 10                      | 6 / 18                   | 18                     | 18/50/0        |
| LibriSpeech straddle, Tocumen bed -12 dB | band [0.30, thr) need 1 | A (p-300)                 | 80     | 335         | 68        | 50               | 1                       | 6 / 17                   | 26                     | 0/65/3         |
| LibriSpeech straddle, Tocumen bed -12 dB | band [0.30, thr) need 1 | B (p-200)                 | 80     | 335         | 68        | 55               | 5                       | 6 / 17                   | 20                     | 0/65/3         |
| LibriSpeech straddle, Tocumen bed -12 dB | band [0.30, thr) need 1 | A' (stage 1 clamped to A) | 80     | 335         | 68        | 49               | 1                       | 6 / 18                   | 42                     | 18/47/3        |
| LibriSpeech straddle, Tocumen bed -12 dB | band [0.30, thr) need 1 | B' (stage 1 clamped to B) | 80     | 335         | 68        | 52               | 4                       | 6 / 19                   | 35                     | 18/47/3        |
| LibriSpeech straddle, clean              | today (no re-check)     | today                     | 118    | 496         | 102       | 65               | 15                      | 0 / 0                    | 9                      | 19/82/1        |
| LibriSpeech straddle, clean              | today (no re-check)     | A (p-300)                 | 118    | 496         | 102       | 56               | 6                       | 0 / 0                    | 17                     | 0/101/1        |
| LibriSpeech straddle, clean              | today (no re-check)     | B (p-200)                 | 118    | 496         | 102       | 59               | 9                       | 0 / 0                    | 7                      | 0/101/1        |
| LibriSpeech straddle, clean              | today (no re-check)     | A' (stage 1 clamped to A) | 118    | 496         | 102       | 55               | 5                       | 0 / 0                    | 21                     | 19/82/1        |
| LibriSpeech straddle, clean              | today (no re-check)     | B' (stage 1 clamped to B) | 118    | 496         | 102       | 58               | 8                       | 0 / 0                    | 12                     | 19/82/1        |
| LibriSpeech straddle, clean              | band [0.30, thr) need 1 | today                     | 118    | 496         | 110       | 47               | 21                      | 8 / 24                   | 14                     | 22/87/1        |
| LibriSpeech straddle, clean              | band [0.30, thr) need 1 | A (p-300)                 | 118    | 496         | 110       | 34               | 8                       | 8 / 24                   | 27                     | 0/109/1        |
| LibriSpeech straddle, clean              | band [0.30, thr) need 1 | B (p-200)                 | 118    | 496         | 110       | 37               | 11                      | 8 / 24                   | 14                     | 0/109/1        |
| LibriSpeech straddle, clean              | band [0.30, thr) need 1 | A' (stage 1 clamped to A) | 118    | 496         | 110       | 36               | 10                      | 8 / 24                   | 35                     | 22/87/1        |
| LibriSpeech straddle, clean              | band [0.30, thr) need 1 | B' (stage 1 clamped to B) | 118    | 496         | 110       | 40               | 14                      | 8 / 24                   | 24                     | 22/87/1        |

### Totals over all held-out sets (lost words, leaked words)

| gate             | cut                       | lost | leaked |
| ---------------- | ------------------------- | ---- | ------ |
| band 0.30 need 1 | A (p-300)                 | 104  | 257    |
| band 0.30 need 1 | A' (stage 1 clamped to A) | 104  | 283    |
| band 0.30 need 1 | B' (stage 1 clamped to B) | 116  | 182    |
| band 0.30 need 1 | B (p-200)                 | 118  | 153    |
| band 0.30 need 1 | today                     | 155  | 71     |
| today gate       | A' (stage 1 clamped to A) | 159  | 180    |
| today gate       | A (p-300)                 | 161  | 167    |
| today gate       | B' (stage 1 clamped to B) | 170  | 110    |
| today gate       | B (p-200)                 | 174  | 94     |
| today gate       | today                     | 216  | 56     |

False accepts of the near-miss re-check (bystander-only utterances confirmed as
the caller): 0/322 and 0/330 on LibriSpeech dev-clean, 0/120 on Piper VAD-separate
bystanders, +1/250 on the bystander part of merged Piper utterances.
Sub-window embeds at 750/125: p99 19 per scan, about 150-250 ms.
Paused merged scenes: 0 of 156 split, 0 leaked words.

## 6j: U1 evidence for an other-to-caller split (decision aa)

Run: `near_miss_bench --evidence` with `--stt`, `--fixtures` and `--straddle-piper` set to the rerolled Piper held-out fixtures, `--librispeech --ls-split dev-clean` and `--bed` (Tocumen). The gate is the shipped one: window 1 `other`, then the first `caller` window or a near-miss window whose 750/125 scan finds a `caller` sub-window within the remaining verify budget, cut by config A. A rule decides from U1's window scores (windows 1..k-1) whether the change splits or stays the re-check upgrade (U1 becomes caller).

- Bystander-to-caller: the 6g held-out scenes (merged no-pause, merged paused, Piper and LibriSpeech straddle). Paused scenes are VAD-separate and never split, so they do not appear.
- Caller-only: LibriSpeech dev-clean whole utterances (6 per caller, not the straddle pieces), Tocumen -12 dB and clean; Piper: 3 caller phrases of consecutive fixture scenes joined with 150 ms gaps, as is and with Tocumen added. Window 1 starts at the caller's onset.
- Lost words are measure A above the floor of the same utterance. The floor is the cut at the caller onset (bystander-to-caller) or at U1's start (caller-only), with no gate. An upgrade loses nothing above the floor and leaks every bystander word of the merged utterance. Whole-utterance drops (window 1 `other`, no confirm) are the same under every rule and are listed apart.
- The Piper caller-only floor is high (6.5 % of reference words) because the reference is the whole joined utterance and the head decode covers 2500 ms; it cancels above the floor.
- R4 (window 1 outside the near-miss band, or a second `other` window) gives the same answer as R3 on every input and has no row of its own.

### Floor and drops per set (no rule changes these)

| set                                                    | utterances | reference words | STT floor (words measure A misses with no gate) | whole-utterance drops (window 1 other, no confirm): utterances / words above floor | other-to-caller splits today |
| ------------------------------------------------------ | ---------- | --------------- | ----------------------------------------------- | ---------------------------------------------------------------------------------- | ---------------------------- |
| Piper straddle                                         | 355        | 1298            | 0                                               | 0 / 0                                                                              | 355                          |
| LibriSpeech straddle, Tocumen bed -12 dB               | 80         | 345             | 10                                              | 11 / 41                                                                            | 69                           |
| LibriSpeech straddle, clean                            | 118        | 496             | 0                                               | 8 / 26                                                                             | 110                          |
| Piper fixtures, merged no-pause                        | 250        | 2881            | 0                                               | 0 / 0                                                                              | 250                          |
| Piper caller-only, fixture babble                      | 600        | 19211           | 1243                                            | 0 / 0                                                                              | 0                            |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | 601        | 19205           | 1233                                            | 0 / 0                                                                              | 4                            |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | 261        | 4698            | 26                                              | 21 / 220                                                                           | 26                           |
| LibriSpeech dev-clean caller-only, clean               | 252        | 4747            | 0                                               | 16 / 190                                                                           | 9                            |

### Per set (split lost above floor / leaked words / splits kept / splits upgraded)

| rule                                                 | Piper straddle     | LibriSpeech straddle, Tocumen bed -12 dB | LibriSpeech straddle, clean | Piper fixtures, merged no-pause | Piper caller-only, fixture babble | Piper caller-only, fixture babble + Tocumen bed -12 dB | LibriSpeech dev-clean caller-only, Tocumen bed -12 dB | LibriSpeech dev-clean caller-only, clean |
| ---------------------------------------------------- | ------------------ | ---------------------------------------- | --------------------------- | ------------------------------- | --------------------------------- | ------------------------------------------------------ | ----------------------------------------------------- | ---------------------------------------- |
| today: always split                                  | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 44 / 250 / 0               | 0 / 0 / 0 / 0                     | 3 / 0 / 4 / 0                                          | 98 / 0 / 26 / 0                                       | 51 / 0 / 9 / 0                           |
| never split (always upgrade)                         | 0 / 4325 / 0 / 355 | 0 / 789 / 0 / 69                         | 0 / 1284 / 0 / 110          | 0 / 4257 / 0 / 250              | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 0 / 0 / 0 / 26                                        | 0 / 0 / 0 / 9                            |
| R2: U1 has >= 2 other windows                        | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 55 / 248 / 2               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 62 / 0 / 5 / 21                                       | 35 / 0 / 3 / 6                           |
| R1': a U1 window < 0.20                              | 0 / 396 / 329 / 26 | 4 / 110 / 62 / 7                         | 6 / 131 / 101 / 9           | 18 / 477 / 223 / 27             | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 0 / 0 / 0 / 26                                        | 17 / 0 / 1 / 8                           |
| R3': a U1 window < 0.20, or >= 2 other windows       | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 55 / 248 / 2               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 62 / 0 / 5 / 21                                       | 35 / 0 / 3 / 6                           |
| R1': a U1 window < 0.25                              | 0 / 166 / 349 / 6  | 4 / 72 / 65 / 4                          | 6 / 85 / 104 / 6            | 19 / 224 / 238 / 12             | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 0 / 0 / 0 / 26                                        | 19 / 0 / 2 / 7                           |
| R3': a U1 window < 0.25, or >= 2 other windows       | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 55 / 248 / 2               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 62 / 0 / 5 / 21                                       | 35 / 0 / 3 / 6                           |
| R1: a U1 window < 0.30                               | 0 / 128 / 352 / 3  | 4 / 26 / 68 / 1                          | 6 / 42 / 108 / 2            | 21 / 44 / 250 / 0               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 46 / 0 / 6 / 20                                       | 19 / 0 / 2 / 7                           |
| R3 (= R4): a U1 window < 0.30, or >= 2 other windows | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 44 / 250 / 0               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 68 / 0 / 8 / 18                                       | 35 / 0 / 3 / 6                           |
| R1': a U1 window < 0.35                              | 0 / 93 / 355 / 0   | 4 / 26 / 68 / 1                          | 6 / 17 / 110 / 0            | 21 / 44 / 250 / 0               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 52 / 0 / 8 / 18                                       | 35 / 0 / 3 / 6                           |
| R3': a U1 window < 0.35, or >= 2 other windows       | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 44 / 250 / 0               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 68 / 0 / 9 / 17                                       | 35 / 0 / 3 / 6                           |
| R1': a U1 window < 0.40                              | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 44 / 250 / 0               | 0 / 0 / 0 / 0                     | 0 / 0 / 1 / 3                                          | 82 / 0 / 16 / 10                                      | 46 / 0 / 6 / 3                           |
| R3': a U1 window < 0.40, or >= 2 other windows       | 0 / 93 / 355 / 0   | 4 / 15 / 69 / 0                          | 6 / 17 / 110 / 0            | 21 / 44 / 250 / 0               | 0 / 0 / 0 / 0                     | 0 / 0 / 1 / 3                                          | 82 / 0 / 16 / 10                                      | 46 / 0 / 6 / 3                           |

### Ranked (split-dependent lost words above the floor, both sets; then leaked words)

| rank | rule                                                 | caller-only lost above floor | caller-only false splits left | bystander->caller lost above floor | true splits upgraded | leaked words | total lost above floor |
| ---- | ---------------------------------------------------- | ---------------------------- | ----------------------------- | ---------------------------------- | -------------------- | ------------ | ---------------------- |
| 1    | never split (always upgrade)                         | 0                            | 0                             | 0                                  | 784                  | 10655        | 0                      |
| 2    | R1': a U1 window < 0.20                              | 17                           | 1                             | 28                                 | 69                   | 1114         | 45                     |
| 3    | R1': a U1 window < 0.25                              | 19                           | 2                             | 29                                 | 28                   | 547          | 48                     |
| 4    | R1: a U1 window < 0.30                               | 65                           | 8                             | 31                                 | 6                    | 240          | 96                     |
| 5    | R1': a U1 window < 0.35                              | 87                           | 11                            | 31                                 | 1                    | 180          | 118                    |
| 6    | R2: U1 has >= 2 other windows                        | 97                           | 8                             | 31                                 | 2                    | 180          | 128                    |
| 7    | R3': a U1 window < 0.20, or >= 2 other windows       | 97                           | 8                             | 31                                 | 2                    | 180          | 128                    |
| 8    | R3': a U1 window < 0.25, or >= 2 other windows       | 97                           | 8                             | 31                                 | 2                    | 180          | 128                    |
| 9    | R3 (= R4): a U1 window < 0.30, or >= 2 other windows | 103                          | 11                            | 31                                 | 0                    | 169          | 134                    |
| 10   | R3': a U1 window < 0.35, or >= 2 other windows       | 103                          | 12                            | 31                                 | 0                    | 169          | 134                    |
| 11   | R1': a U1 window < 0.40                              | 128                          | 23                            | 31                                 | 0                    | 169          | 159                    |
| 12   | R3': a U1 window < 0.40, or >= 2 other windows       | 128                          | 23                            | 31                                 | 0                    | 169          | 159                    |
| 13   | today: always split                                  | 152                          | 39                            | 31                                 | 0                    | 169          | 183                    |

### Caller-only false splits left under "R1': a U1 window < 0.25" (set, U1 scores, caller words lost above floor)

- LibriSpeech dev-clean caller-only, clean: U1 [0.221,0.413], lost 2
- LibriSpeech dev-clean caller-only, clean: U1 [0.370,0.215,0.190,0.306], lost 17

### Caller-only false splits left under "R1: a U1 window < 0.30" (set, U1 scores, caller words lost above floor)

- LibriSpeech dev-clean caller-only, Tocumen bed -12 dB: U1 [0.275], lost 2
- LibriSpeech dev-clean caller-only, Tocumen bed -12 dB: U1 [0.297,0.441], lost 5
- LibriSpeech dev-clean caller-only, Tocumen bed -12 dB: U1 [0.426,0.282,0.332,0.455,0.308,0.412], lost 20
- LibriSpeech dev-clean caller-only, Tocumen bed -12 dB: U1 [0.276], lost 4
- LibriSpeech dev-clean caller-only, Tocumen bed -12 dB: U1 [0.300,0.418,0.381], lost 15
- LibriSpeech dev-clean caller-only, Tocumen bed -12 dB: U1 [0.298], lost 0
- LibriSpeech dev-clean caller-only, clean: U1 [0.221,0.413], lost 2
- LibriSpeech dev-clean caller-only, clean: U1 [0.370,0.215,0.190,0.306], lost 17

Gate false accepts are unchanged by every rule: a rule only decides split or upgrade after the gate already confirmed `caller`. An upgrade counts no lost words above the floor; its leak is every bystander word of the merged utterance.

## Chunk 7f: scan-miss veto for other -> caller (decision gg), not shipped

Held-out 6g sets, `--evidence`, threshold 0.459, shipped split rule (decision bb). Modes: today; veto (a `caller` window whose split scan completes with no `caller` sub-window does not confirm, the watch stays open); narrow veto (the same only below threshold + 0.05). Run 2262 s on this Mac, log `.test-logs/20261115-222456-7f-bench.log` (not committed).

Result: the bar fails for both modes. Lost caller words rise on every LibriSpeech set (straddle bed 45 -> 51, clean 32 -> 35; caller-only bed 220 -> 248, clean 207 -> 213), mostly from utterances whose only confirming window was vetoed and never confirmed again ("no confirm"), plus delayed confirms. The n02 shape (confirm more than 1500 ms before the caller onset) goes from 6 + 4 to 0 + 0 on Piper (veto) and 4 + 1 (narrow). Bystander-only false splits are unchanged (0 / 1).

| set                                                    | mode                       | lost (A) | leaked | same window | delayed 1 / 2+ | no confirm | split->upgrade / upgrade->split | caller-only false splits | early confirms | scans / embeds / ms / max ms |
| ------------------------------------------------------ | -------------------------- | -------- | ------ | ----------- | -------------- | ---------- | ------------------------------- | ------------------------ | -------------- | ---------------------------- |
| Piper straddle                                         | today                      | 0        | 396    | 355         | 0 / 0          | 0          | 0 / 0                           | -                        | 6              | 380 / 5793 / 45293 / 154     |
| Piper straddle                                         | veto                       | 0        | 358    | 349         | 6 / 0          | 0          | 0 / 0                           | -                        | 0              | 386 / 5842 / 45673 / 154     |
| Piper straddle                                         | narrow veto (< thr + 0.05) | 0        | 385    | 353         | 2 / 0          | 0          | 0 / 0                           | -                        | 4              | 382 / 5810 / 45423 / 154     |
| LibriSpeech straddle, Tocumen bed -12 dB               | today                      | 45       | 110    | 69          | 0 / 0          | 0          | 0 / 0                           | -                        | 1              | 85 / 1191 / 9447 / 156       |
| LibriSpeech straddle, Tocumen bed -12 dB               | veto                       | 51       | 104    | 66          | 1 / 0          | 2          | 0 / 0                           | -                        | 0              | 86 / 1199 / 9508 / 156       |
| LibriSpeech straddle, Tocumen bed -12 dB               | narrow veto (< thr + 0.05) | 51       | 104    | 66          | 1 / 0          | 2          | 0 / 0                           | -                        | 0              | 86 / 1199 / 9508 / 156       |
| LibriSpeech straddle, clean                            | today                      | 32       | 131    | 110         | 0 / 0          | 0          | 0 / 0                           | -                        | 0              | 125 / 1606 / 12780 / 160     |
| LibriSpeech straddle, clean                            | veto                       | 35       | 129    | 109         | 0 / 0          | 1          | 0 / 0                           | -                        | 0              | 125 / 1606 / 12780 / 160     |
| LibriSpeech straddle, clean                            | narrow veto (< thr + 0.05) | 35       | 129    | 109         | 0 / 0          | 1          | 0 / 0                           | -                        | 0              | 125 / 1606 / 12780 / 160     |
| Piper fixtures, merged no-pause                        | today                      | 18       | 477    | 250         | 0 / 0          | 0          | 0 / 0                           | -                        | 4              | 313 / 4345 / 33589 / 153     |
| Piper fixtures, merged no-pause                        | veto                       | 13       | 477    | 246         | 2 / 2          | 0          | 0 / 0                           | -                        | 0              | 317 / 4386 / 33908 / 153     |
| Piper fixtures, merged no-pause                        | narrow veto (< thr + 0.05) | 13       | 477    | 247         | 1 / 2          | 0          | 0 / 0                           | -                        | 1              | 316 / 4377 / 33838 / 153     |
| Piper caller-only, fixture babble                      | today                      | 0        | 0      | 0           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0 / 0 / 0 / 0                |
| Piper caller-only, fixture babble                      | veto                       | 0        | 0      | 0           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0 / 0 / 0 / 0                |
| Piper caller-only, fixture babble                      | narrow veto (< thr + 0.05) | 0        | 0      | 0           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0 / 0 / 0 / 0                |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | today                      | 0        | 0      | 4           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 4 / 35 / 277 / 79            |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | veto                       | 0        | 0      | 4           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 4 / 35 / 277 / 79            |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | narrow veto (< thr + 0.05) | 0        | 0      | 4           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 4 / 35 / 277 / 79            |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | today                      | 220      | 0      | 26          | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 54 / 866 / 6725 / 152        |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | veto                       | 248      | 0      | 21          | 3 / 0          | 2          | 0 / 0                           | 0                        | 0              | 58 / 921 / 7149 / 152        |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | narrow veto (< thr + 0.05) | 248      | 0      | 22          | 2 / 0          | 2          | 0 / 0                           | 0                        | 0              | 57 / 906 / 7031 / 152        |
| LibriSpeech dev-clean caller-only, clean               | today                      | 207      | 0      | 9           | 0 / 0          | 0          | 0 / 0                           | 1                        | 0              | 29 / 500 / 3867 / 151        |
| LibriSpeech dev-clean caller-only, clean               | veto                       | 213      | 0      | 8           | 0 / 0          | 1          | 0 / 0                           | 1                        | 0              | 29 / 500 / 3867 / 151        |
| LibriSpeech dev-clean caller-only, clean               | narrow veto (< thr + 0.05) | 213      | 0      | 8           | 0 / 0          | 1          | 0 / 0                           | 1                        | 0              | 29 / 500 / 3867 / 151        |

### Bystander-only false splits (LibriSpeech bystander-only utterances, window 1 `other`, 2+ windows)

| set                                  | eligible | today | veto | narrow veto (< thr + 0.05) |
| ------------------------------------ | -------- | ----- | ---- | -------------------------- |
| LS bystander-only Tocumen bed -12 dB | 322      | 0     | 0    | 0                          |
| LS bystander-only clean              | 330      | 1     | 1    | 1                          |

### Deferred veto (decision hh), picked

Mode 3: the vetoed window is remembered with its fallback cut; a later window with a `caller` sub-window splits there; a second `caller` window with no hit cuts at the remembered fallback; the end of speech with the veto pending falls back to `caller` whole (late barge). Same sets, run 2322 s, log `.test-logs/*-7f-bench-hh.log` (not committed).

Bar of decision gg holds: lost caller words are at or below today on every set (LS straddle bed 45 / 45, clean 32 / 32; LS caller-only bed 220 / 220, clean 207 / 207; Piper merged no-pause 18 -> 13; Piper 0 / 0); bystander-only false splits unchanged (0 / 322, 1 / 330), no bystander-only utterance ends with the veto pending, caller-labelled bystander words unchanged (0 / 0, 9 / 9); early cuts of the n02 shape on Piper 6 + 4 -> 0 + 0. Cost: the LS utterances that end pending (bed 2 + 2, clean 1 + 1) become `caller` whole, with the barge at the end of speech, so leaked words rise (LS straddle bed 110 -> 130, clean 131 -> 140); decision w takes leaks over lost words.

| set                                                    | mode                       | lost (A) | leaked | same window | delayed 1 / 2+ | no confirm | split->upgrade / upgrade->split | caller-only false splits | early confirms | end of speech with the veto pending -> whole utterance `caller` (late barge) | scans / embeds / ms / max ms |
| ------------------------------------------------------ | -------------------------- | -------- | ------ | ----------- | -------------- | ---------- | ------------------------------- | ------------------------ | -------------- | ---------------------------------------------------------------------------- | ---------------------------- |
| Piper straddle                                         | today                      | 0        | 396    | 355         | 0 / 0          | 0          | 0 / 0                           | -                        | 6              | 0                                                                            | 380 / 5793 / 44621 / 153     |
| Piper straddle                                         | veto                       | 0        | 358    | 349         | 6 / 0          | 0          | 0 / 0                           | -                        | 0              | 0                                                                            | 386 / 5842 / 44997 / 153     |
| Piper straddle                                         | narrow veto (< thr + 0.05) | 0        | 385    | 353         | 2 / 0          | 0          | 0 / 0                           | -                        | 4              | 0                                                                            | 382 / 5810 / 44752 / 153     |
| Piper straddle                                         | deferred veto (hh)         | 0        | 358    | 349         | 6 / 0          | 0          | 0 / 0                           | -                        | 0              | 0                                                                            | 386 / 5842 / 44997 / 153     |
| LibriSpeech straddle, Tocumen bed -12 dB               | today                      | 45       | 110    | 69          | 0 / 0          | 0          | 0 / 0                           | -                        | 1              | 0                                                                            | 85 / 1191 / 9429 / 184       |
| LibriSpeech straddle, Tocumen bed -12 dB               | veto                       | 51       | 104    | 66          | 1 / 0          | 2          | 0 / 0                           | -                        | 0              | 0                                                                            | 86 / 1199 / 9494 / 184       |
| LibriSpeech straddle, Tocumen bed -12 dB               | narrow veto (< thr + 0.05) | 51       | 104    | 66          | 1 / 0          | 2          | 0 / 0                           | -                        | 0              | 0                                                                            | 86 / 1199 / 9494 / 184       |
| LibriSpeech straddle, Tocumen bed -12 dB               | deferred veto (hh)         | 45       | 130    | 66          | 1 / 0          | 0          | 0 / 0                           | -                        | 0              | 2                                                                            | 86 / 1199 / 9494 / 184       |
| LibriSpeech straddle, clean                            | today                      | 32       | 131    | 110         | 0 / 0          | 0          | 0 / 0                           | -                        | 0              | 0                                                                            | 125 / 1606 / 12932 / 163     |
| LibriSpeech straddle, clean                            | veto                       | 35       | 129    | 109         | 0 / 0          | 1          | 0 / 0                           | -                        | 0              | 0                                                                            | 125 / 1606 / 12932 / 163     |
| LibriSpeech straddle, clean                            | narrow veto (< thr + 0.05) | 35       | 129    | 109         | 0 / 0          | 1          | 0 / 0                           | -                        | 0              | 0                                                                            | 125 / 1606 / 12932 / 163     |
| LibriSpeech straddle, clean                            | deferred veto (hh)         | 32       | 140    | 109         | 0 / 0          | 0          | 0 / 0                           | -                        | 0              | 1                                                                            | 125 / 1606 / 12932 / 163     |
| Piper fixtures, merged no-pause                        | today                      | 18       | 477    | 250         | 0 / 0          | 0          | 0 / 0                           | -                        | 4              | 0                                                                            | 313 / 4345 / 36179 / 260     |
| Piper fixtures, merged no-pause                        | veto                       | 13       | 477    | 246         | 2 / 2          | 0          | 0 / 0                           | -                        | 0              | 0                                                                            | 317 / 4386 / 36516 / 260     |
| Piper fixtures, merged no-pause                        | narrow veto (< thr + 0.05) | 13       | 477    | 247         | 1 / 2          | 0          | 0 / 0                           | -                        | 1              | 0                                                                            | 316 / 4377 / 36442 / 260     |
| Piper fixtures, merged no-pause                        | deferred veto (hh)         | 13       | 477    | 246         | 2 / 2          | 0          | 0 / 0                           | -                        | 0              | 0                                                                            | 317 / 4386 / 36516 / 260     |
| Piper caller-only, fixture babble                      | today                      | 0        | 0      | 0           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 0 / 0 / 0 / 0                |
| Piper caller-only, fixture babble                      | veto                       | 0        | 0      | 0           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 0 / 0 / 0 / 0                |
| Piper caller-only, fixture babble                      | narrow veto (< thr + 0.05) | 0        | 0      | 0           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 0 / 0 / 0 / 0                |
| Piper caller-only, fixture babble                      | deferred veto (hh)         | 0        | 0      | 0           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 0 / 0 / 0 / 0                |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | today                      | 0        | 0      | 4           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 4 / 35 / 281 / 80            |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | veto                       | 0        | 0      | 4           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 4 / 35 / 281 / 80            |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | narrow veto (< thr + 0.05) | 0        | 0      | 4           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 4 / 35 / 281 / 80            |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | deferred veto (hh)         | 0        | 0      | 4           | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 4 / 35 / 281 / 80            |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | today                      | 220      | 0      | 26          | 0 / 0          | 0          | 0 / 0                           | 0                        | 0              | 0                                                                            | 54 / 866 / 6748 / 152        |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | veto                       | 248      | 0      | 21          | 3 / 0          | 2          | 0 / 0                           | 0                        | 0              | 0                                                                            | 58 / 921 / 7176 / 152        |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | narrow veto (< thr + 0.05) | 248      | 0      | 22          | 2 / 0          | 2          | 0 / 0                           | 0                        | 0              | 0                                                                            | 57 / 906 / 7058 / 152        |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB  | deferred veto (hh)         | 220      | 0      | 21          | 3 / 0          | 0          | 0 / 0                           | 0                        | 0              | 2                                                                            | 58 / 921 / 7176 / 152        |
| LibriSpeech dev-clean caller-only, clean               | today                      | 207      | 0      | 9           | 0 / 0          | 0          | 0 / 0                           | 1                        | 0              | 0                                                                            | 29 / 500 / 3877 / 152        |
| LibriSpeech dev-clean caller-only, clean               | veto                       | 213      | 0      | 8           | 0 / 0          | 1          | 0 / 0                           | 1                        | 0              | 0                                                                            | 29 / 500 / 3877 / 152        |
| LibriSpeech dev-clean caller-only, clean               | narrow veto (< thr + 0.05) | 213      | 0      | 8           | 0 / 0          | 1          | 0 / 0                           | 1                        | 0              | 0                                                                            | 29 / 500 / 3877 / 152        |
| LibriSpeech dev-clean caller-only, clean               | deferred veto (hh)         | 207      | 0      | 8           | 0 / 0          | 0          | 0 / 0                           | 1                        | 0              | 1                                                                            | 29 / 500 / 3877 / 152        |

### Bystander-only false splits (LibriSpeech bystander-only utterances, window 1 `other`, 2+ windows)

Confirms per mode; deferred veto: utterances that end with the veto pending (whole utterance `caller`); bystander words labelled `caller` (today: after the split cut; deferred veto: after its cut, or the whole utterance when it ends pending).

| set                                  | eligible | today | veto | narrow veto (< thr + 0.05) | deferred veto (hh) | deferred: end pending -> caller | bystander words labelled caller, today / deferred |
| ------------------------------------ | -------- | ----- | ---- | -------------------------- | ------------------ | ------------------------------- | ------------------------------------------------- |
| LS bystander-only Tocumen bed -12 dB | 322      | 0     | 0    | 0                          | 0                  | 0                               | 0 / 0                                             |
| LS bystander-only clean              | 330      | 1     | 1    | 1                          | 1                  | 0                               | 9 / 9                                             |

## Chunk 7h: second cut on a split-born U2 (decision jj), shipped

`--second-cut` scenes: every straddle scene whose U1 splits other -> caller under the shipped deferred veto (`product_o2c` mode 3, decision hh) gets a simulated U2: its caller floor (`caller_floor`, decision y), the product's U2 window grid (the seed window scored at the split, then U1's 1500 ms grid) and the caller->other watch run on it. The cut goes through the product function (`speaker_gate::bench_caller_to_other_cut`). Rules: **today** (every `other` window counts); **jj** (a window whose middle lies before the floor resets the run like a `caller` window; the shipped rule); **option 2** (today's count, but a cut that would land at the floor moves forward to the end of the next closed quiet run). Tail scenes are the plain straddle sets (the caller phrase, then 2 s of bed or silence). Takeover scenes (`--takeover-db <dB>`) append the head of the bystander clip (up to 2500 ms) after the caller phrase at that gain. Caller words cut = recognized caller words in the whole U2 minus those in U2 up to the cut (LCS against the caller-alone reference, sherpa streaming zipformer); bystander-2 words left = recognized takeover words still in U2 before the cut.

Run (models from the primary checkout's `examples/voice-agent-local-sherpa/.models`, LibriSpeech under `/tmp/librispeech/LibriSpeech`, bed `/tmp/sg-demo/bed/tocumen48.f32`, Piper straddle fixtures `/tmp/sg-nm-n400` and `/tmp/sg-nm-p200`; audio is never committed):

```bash
cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example near_miss_bench -- \
  --model $M/speaker/nemo_en_titanet_small.onnx --stt $M/sherpa-onnx-streaming-zipformer-en-2023-06-26 \
  --second-cut --librispeech /tmp/librispeech/LibriSpeech [--ls-split dev-clean] --bed /tmp/sg-demo/bed/tocumen48.f32 [--takeover-db -3] --out /tmp/7h.md
```

Result: the bar of decision jj holds. On the tail scenes, caller words cut out of U2 go from 3 (1 scene, LibriSpeech test-clean, the n03 shape) to 0: that U2 is not split and stays `caller` whole. The held-out dev-clean tail sets have no caller->other split under any rule (0 -> 0). On the takeover sets jj cuts fewer caller words in every set (29 -> 28, 61 -> 57, 25 -> 24, 61 -> 57, 18 -> 17, 62 -> 58), leaves at most one more bystander-2 word (133 -> 134, 123 -> 124) and splits 1-4 takeovers one window later; no takeover goes unsplit. On the held-out dev-clean takeover sets jj cuts no more caller words than today on any set (bed 32 -> 31, 19 -> 18, 12 -> 12; clean 44 -> 44, 41 -> 41, 40 -> 40); on the bed sets it leaves 8 more bystander-2 words each (65 -> 73, 82 -> 90, 93 -> 101) and splits 2 takeovers one window later; no takeover goes unsplit. Option 2 removes no tail cut (3 -> 3) and helps takeovers less, so it is not shipped. Piper has no takeover scenes: its straddle scenes are the prebuilt mixed fixtures (`sweep_fixture`), whose caller and bystander pieces are cut out of the scene with its babble bed, so there is no clean bystander clip to append after the caller phrase.

| set                                                                           | rule                                           | U2s | caller -> other splits | caller words cut out of U2 (scenes) | bystander-2 words left in U2 / total | split later than today / not split |
| ----------------------------------------------------------------------------- | ---------------------------------------------- | --- | ---------------------- | ----------------------------------- | ------------------------------------ | ---------------------------------- |
| Piper straddle                                                                | today                                          | 398 | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| Piper straddle                                                                | jj: pre-floor windows reset the run            | 398 | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| Piper straddle                                                                | option 2: forward to the next closed quiet run | 398 | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| test-clean LibriSpeech straddle, Tocumen bed -12 dB                           | today                                          | 75  | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| test-clean LibriSpeech straddle, Tocumen bed -12 dB                           | jj: pre-floor windows reset the run            | 75  | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| test-clean LibriSpeech straddle, Tocumen bed -12 dB                           | option 2: forward to the next closed quiet run | 75  | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| test-clean LibriSpeech straddle, clean                                        | today                                          | 109 | 1                      | 3 (1)                               | - / -                                | 0 / 0                              |
| test-clean LibriSpeech straddle, clean                                        | jj: pre-floor windows reset the run            | 109 | 0                      | 0 (0)                               | - / -                                | 0 / 1                              |
| test-clean LibriSpeech straddle, clean                                        | option 2: forward to the next closed quiet run | 109 | 1                      | 3 (1)                               | - / -                                | 0 / 0                              |
| dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB                 | today                                          | 67  | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB                 | jj: pre-floor windows reset the run            | 67  | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB                 | option 2: forward to the next closed quiet run | 67  | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| dev-clean (held out) LibriSpeech straddle, clean                              | today                                          | 109 | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| dev-clean (held out) LibriSpeech straddle, clean                              | jj: pre-floor windows reset the run            | 109 | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| dev-clean (held out) LibriSpeech straddle, clean                              | option 2: forward to the next closed quiet run | 109 | 0                      | 0 (0)                               | - / -                                | 0 / 0                              |
| takeover 0 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB            | today                                          | 63  | 56                     | 29 (23)                             | 92 / 480                             | 0 / 0                              |
| takeover 0 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB            | jj: pre-floor windows reset the run            | 63  | 56                     | 28 (22)                             | 92 / 480                             | 1 / 0                              |
| takeover 0 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB            | option 2: forward to the next closed quiet run | 63  | 56                     | 28 (22)                             | 92 / 480                             | 1 / 0                              |
| takeover 0 dB, test-clean LibriSpeech straddle, clean                         | today                                          | 94  | 79                     | 61 (38)                             | 133 / 728                            | 0 / 0                              |
| takeover 0 dB, test-clean LibriSpeech straddle, clean                         | jj: pre-floor windows reset the run            | 94  | 79                     | 57 (36)                             | 134 / 728                            | 4 / 0                              |
| takeover 0 dB, test-clean LibriSpeech straddle, clean                         | option 2: forward to the next closed quiet run | 94  | 79                     | 60 (37)                             | 134 / 728                            | 3 / 0                              |
| takeover -3 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB           | today                                          | 65  | 53                     | 25 (19)                             | 97 / 492                             | 0 / 0                              |
| takeover -3 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB           | jj: pre-floor windows reset the run            | 65  | 53                     | 24 (18)                             | 97 / 492                             | 1 / 0                              |
| takeover -3 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB           | option 2: forward to the next closed quiet run | 65  | 53                     | 24 (18)                             | 97 / 492                             | 1 / 0                              |
| takeover -3 dB, test-clean LibriSpeech straddle, clean                        | today                                          | 93  | 79                     | 61 (37)                             | 123 / 717                            | 0 / 0                              |
| takeover -3 dB, test-clean LibriSpeech straddle, clean                        | jj: pre-floor windows reset the run            | 93  | 79                     | 57 (35)                             | 124 / 717                            | 4 / 0                              |
| takeover -3 dB, test-clean LibriSpeech straddle, clean                        | option 2: forward to the next closed quiet run | 93  | 79                     | 60 (36)                             | 124 / 717                            | 3 / 0                              |
| takeover -6 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB           | today                                          | 66  | 45                     | 18 (15)                             | 118 / 497                            | 0 / 0                              |
| takeover -6 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB           | jj: pre-floor windows reset the run            | 66  | 45                     | 17 (14)                             | 118 / 497                            | 1 / 0                              |
| takeover -6 dB, test-clean LibriSpeech straddle, Tocumen bed -12 dB           | option 2: forward to the next closed quiet run | 66  | 45                     | 17 (14)                             | 118 / 497                            | 1 / 0                              |
| takeover -6 dB, test-clean LibriSpeech straddle, clean                        | today                                          | 94  | 78                     | 62 (36)                             | 130 / 719                            | 0 / 0                              |
| takeover -6 dB, test-clean LibriSpeech straddle, clean                        | jj: pre-floor windows reset the run            | 94  | 78                     | 58 (34)                             | 130 / 719                            | 4 / 0                              |
| takeover -6 dB, test-clean LibriSpeech straddle, clean                        | option 2: forward to the next closed quiet run | 94  | 78                     | 61 (35)                             | 130 / 719                            | 3 / 0                              |
| takeover 0 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB  | today                                          | 57  | 48                     | 32 (21)                             | 65 / 452                             | 0 / 0                              |
| takeover 0 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB  | jj: pre-floor windows reset the run            | 57  | 48                     | 31 (20)                             | 73 / 452                             | 2 / 0                              |
| takeover 0 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB  | option 2: forward to the next closed quiet run | 57  | 48                     | 32 (21)                             | 65 / 452                             | 0 / 0                              |
| takeover 0 dB, dev-clean (held out) LibriSpeech straddle, clean               | today                                          | 89  | 70                     | 44 (29)                             | 164 / 716                            | 0 / 0                              |
| takeover 0 dB, dev-clean (held out) LibriSpeech straddle, clean               | jj: pre-floor windows reset the run            | 89  | 70                     | 44 (29)                             | 164 / 716                            | 0 / 0                              |
| takeover 0 dB, dev-clean (held out) LibriSpeech straddle, clean               | option 2: forward to the next closed quiet run | 89  | 70                     | 44 (29)                             | 164 / 716                            | 0 / 0                              |
| takeover -3 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB | today                                          | 57  | 44                     | 19 (14)                             | 82 / 449                             | 0 / 0                              |
| takeover -3 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB | jj: pre-floor windows reset the run            | 57  | 44                     | 18 (13)                             | 90 / 449                             | 2 / 0                              |
| takeover -3 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB | option 2: forward to the next closed quiet run | 57  | 44                     | 19 (14)                             | 82 / 449                             | 0 / 0                              |
| takeover -3 dB, dev-clean (held out) LibriSpeech straddle, clean              | today                                          | 91  | 70                     | 41 (25)                             | 153 / 733                            | 0 / 0                              |
| takeover -3 dB, dev-clean (held out) LibriSpeech straddle, clean              | jj: pre-floor windows reset the run            | 91  | 70                     | 41 (25)                             | 153 / 733                            | 0 / 0                              |
| takeover -3 dB, dev-clean (held out) LibriSpeech straddle, clean              | option 2: forward to the next closed quiet run | 91  | 70                     | 41 (25)                             | 153 / 733                            | 0 / 0                              |
| takeover -6 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB | today                                          | 59  | 42                     | 12 (9)                              | 93 / 458                             | 0 / 0                              |
| takeover -6 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB | jj: pre-floor windows reset the run            | 59  | 42                     | 12 (9)                              | 101 / 458                            | 2 / 0                              |
| takeover -6 dB, dev-clean (held out) LibriSpeech straddle, Tocumen bed -12 dB | option 2: forward to the next closed quiet run | 59  | 42                     | 12 (9)                              | 93 / 458                             | 0 / 0                              |
| takeover -6 dB, dev-clean (held out) LibriSpeech straddle, clean              | today                                          | 91  | 67                     | 40 (25)                             | 166 / 723                            | 0 / 0                              |
| takeover -6 dB, dev-clean (held out) LibriSpeech straddle, clean              | jj: pre-floor windows reset the run            | 91  | 67                     | 40 (25)                             | 166 / 723                            | 0 / 0                              |
| takeover -6 dB, dev-clean (held out) LibriSpeech straddle, clean              | option 2: forward to the next closed quiet run | 91  | 67                     | 40 (25)                             | 166 / 723                            | 0 / 0                              |

## Chunk 7i: bb or a long `other` run (decision kk), not shipped

Run: the 6j command (`near_miss_bench --evidence`, same sets, threshold 0.459) on e5d01c86 with the R5 rows added; hh and jj are on. Today's row (bb, R1' < 0.20) is identical to 6j. R5(N): split if a U1 window is below 0.20 or U1 holds at least N `other` windows; R5'(N) the same with 0.25.

Bar (decision kk), against bb on every held-out set: caller-only lost words above the floor and caller-only false splits do not go up, true splits upgraded and leaked words go down. **No N meets it.**

- N = 5: LibriSpeech dev-clean caller-only with the Tocumen bed goes from 0 to 20 caller words lost and 0 to 1 false split (U1 [0.426,0.282,0.332,0.455,0.308,0.412]). Upgrades and leaks do not move on any bystander set (69 / 1114 in total, the same as bb): no held-out bystander utterance has 5 `other` windows with none below 0.20.
- N = 4: the same set loses 36 words (2 false splits); leaks go down on two sets (LibriSpeech straddle clean, Piper merged no-pause; 897 in total), not on Piper straddle or LibriSpeech straddle with the bed.
- N = 3: 57 words lost on that set (4 false splits) and 33 on dev-clean clean (17 today).
- The 0.25 variants fail the same way on dev-clean (and the clean set adds a 2-word false split, U1 [0.221,0.413]).

### Per set (split lost above floor / leaked words / splits kept / splits upgraded)

| rule                                              | Piper straddle     | LibriSpeech straddle, Tocumen bed -12 dB | LibriSpeech straddle, clean | Piper fixtures, merged no-pause | Piper caller-only, fixture babble | Piper caller-only, fixture babble + Tocumen bed -12 dB | LibriSpeech dev-clean caller-only, Tocumen bed -12 dB | LibriSpeech dev-clean caller-only, clean |
| ------------------------------------------------- | ------------------ | ---------------------------------------- | --------------------------- | ------------------------------- | --------------------------------- | ------------------------------------------------------ | ----------------------------------------------------- | ---------------------------------------- |
| R1': a U1 window < 0.20                           | 0 / 396 / 329 / 26 | 4 / 110 / 62 / 7                         | 6 / 131 / 101 / 9           | 18 / 477 / 223 / 27             | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 0 / 0 / 0 / 26                                        | 17 / 0 / 1 / 8                           |
| R1': a U1 window < 0.25                           | 0 / 166 / 349 / 6  | 4 / 72 / 65 / 4                          | 6 / 85 / 104 / 6            | 19 / 224 / 238 / 12             | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 0 / 0 / 0 / 26                                        | 19 / 0 / 2 / 7                           |
| R5(3): a U1 window < 0.20, or >= 3 other windows  | 0 / 396 / 329 / 26 | 4 / 61 / 65 / 4                          | 6 / 99 / 103 / 7            | 21 / 55 / 248 / 2               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 57 / 0 / 4 / 22                                       | 33 / 0 / 2 / 7                           |
| R5(4): a U1 window < 0.20, or >= 4 other windows  | 0 / 396 / 329 / 26 | 4 / 110 / 62 / 7                         | 6 / 114 / 102 / 8           | 18 / 277 / 235 / 15             | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 36 / 0 / 2 / 24                                       | 17 / 0 / 1 / 8                           |
| R5(5): a U1 window < 0.20, or >= 5 other windows  | 0 / 396 / 329 / 26 | 4 / 110 / 62 / 7                         | 6 / 131 / 101 / 9           | 18 / 477 / 223 / 27             | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 20 / 0 / 1 / 25                                       | 17 / 0 / 1 / 8                           |
| R5'(3): a U1 window < 0.25, or >= 3 other windows | 0 / 166 / 349 / 6  | 4 / 39 / 67 / 2                          | 6 / 85 / 104 / 6            | 21 / 55 / 248 / 2               | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 57 / 0 / 4 / 22                                       | 35 / 0 / 3 / 6                           |
| R5'(4): a U1 window < 0.25, or >= 4 other windows | 0 / 166 / 349 / 6  | 4 / 72 / 65 / 4                          | 6 / 85 / 104 / 6            | 19 / 142 / 243 / 7              | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 36 / 0 / 2 / 24                                       | 19 / 0 / 2 / 7                           |
| R5'(5): a U1 window < 0.25, or >= 5 other windows | 0 / 166 / 349 / 6  | 4 / 72 / 65 / 4                          | 6 / 85 / 104 / 6            | 19 / 224 / 238 / 12             | 0 / 0 / 0 / 0                     | 0 / 0 / 0 / 4                                          | 20 / 0 / 1 / 25                                       | 19 / 0 / 2 / 7                           |

### Ranked (both sets)

| rank | rule                                              | caller-only lost above floor | caller-only false splits left | bystander->caller lost above floor | true splits upgraded | leaked words | total lost above floor |
| ---- | ------------------------------------------------- | ---------------------------- | ----------------------------- | ---------------------------------- | -------------------- | ------------ | ---------------------- |
| 2    | R1': a U1 window < 0.20                           | 17                           | 1                             | 28                                 | 69                   | 1114         | 45                     |
| 3    | R1': a U1 window < 0.25                           | 19                           | 2                             | 29                                 | 28                   | 547          | 48                     |
| 4    | R5(5): a U1 window < 0.20, or >= 5 other windows  | 37                           | 2                             | 28                                 | 69                   | 1114         | 65                     |
| 5    | R5'(5): a U1 window < 0.25, or >= 5 other windows | 39                           | 3                             | 29                                 | 28                   | 547          | 68                     |
| 6    | R5(4): a U1 window < 0.20, or >= 4 other windows  | 53                           | 3                             | 28                                 | 56                   | 897          | 81                     |
| 7    | R5'(4): a U1 window < 0.25, or >= 4 other windows | 55                           | 4                             | 29                                 | 23                   | 465          | 84                     |
| 10   | R5(3): a U1 window < 0.20, or >= 3 other windows  | 90                           | 6                             | 31                                 | 39                   | 611          | 121                    |
| 11   | R5'(3): a U1 window < 0.25, or >= 3 other windows | 92                           | 7                             | 31                                 | 16                   | 345          | 123                    |

### Caller-only false splits under R5(5) (set, U1 scores, caller words lost above floor)

- LibriSpeech dev-clean caller-only, Tocumen bed -12 dB: U1 [0.426,0.282,0.332,0.455,0.308,0.412], lost 20
- LibriSpeech dev-clean caller-only, clean: U1 [0.370,0.215,0.190,0.306], lost 17

So the eu-load n01 shape (five `other` windows 0.213-0.351, about 7 s) stays an upgrade. Product unchanged; the question goes to the user (decision kk).


## Per-utterance caller loss (caller-reject-rate, chunk 1)

Plan `development/node-webrtc-rust/plans/2026-11-16-caller-reject-rate.md`. Bench-only: the new `near_miss_bench --utterance-loss` mode (no product code changed). Measures, with the shipped 0.20.0 rules, how many real caller utterances end up dropped or cut, how many words that costs, and why.

**Gate under test.** Threshold 0.459, `split_on_speaker_change` on (the cloud setting): window 1 `caller` keeps the utterance; window 1 `other` goes through the product other-to-caller path (`product_o2c` mode 3, deferred veto, near-miss scan 750/125) with split rule bb (a U1 window below 0.20 splits, otherwise the change upgrades U1 to `caller`). Then the caller -> other watch runs only where the product runs it (`watch_step`: armed on a first window, or by `Msg::Inherit` on a split-born U2): a whole-caller utterance (window 1 `caller`) is armed from window 1 (floor 0, rule 0); a split-born U2 uses the 7h U2 clock with floor `found.map_or(300, |f| (f + 750) - origin)` and rule 1 (jj, shipped). An upgrade (the change confirms with no confident `other` window, so the watch ends with `done`) and the deferred end-of-speech upgrade (hh, `resolve_deferred_split_at_end`) have no caller -> other watch. Two consecutive `other` windows confirm; the cut comes from `speaker_gate::bench_caller_to_other_cut`, as in `u2_cut`. The caller words cut out of the kept part (LCS of the reference against the decode of the kept part, whole versus up to the cut) are added to the lost words. A cut marks the utterance `partly lost` when it loses at least one word; `caller->other splits` counts every cut, including those that lost none. Each cut is classified by its confirming window (the second `other` window, decision pp), not by the cut point, because the cut lies at most at the middle of the first `other` window and so before the voiced end even when the bed alone confirmed it: (a) confirmed by a bed-tail window (the confirming window's middle is at or after the clean voiced end of the caller speech): real product behaviour, the bed goes on after the caller stops, but not a cut in the middle of speech; (b) confirmed by post-truncation audio (the scene was cut at 15 s and the confirming window ends after that point): a bench artefact, excluded from the product claim; (c) confirmed mid-speech. The deterministic check `classifier_tests::confirming_window_past_voiced_end_is_bed_tail_not_mid_speech` (window n-1 holds the last caller words, window n lies past the voiced end, both `other`) gives cut 3100 ms, confirm 5300-6800 ms, voiced end 5000 ms: class (c) by the old cut-point rule, (a) by the confirming window. A VAD utterance under 1.5 s is `unknown` and fails open (not lost).

**Enrollment (product-faithful).** Whole voiced utterances of the speaker are appended until at least 3000 ms, then one embed of the whole buffer (`enroll_utterance` / `Msg::Enroll`). The bed (Tocumen -12 dB) is on the enrollment audio too in the bed rows. This differs from the earlier benches, which embed exactly 3000 ms from the voiced onset of one clean utterance, so the numbers here are not comparable to 6j.

**Modes.** same-session (primary): enroll from the first utterances of the speaker's first chapter, test on the next 15 utterances of that chapter. cross-session: enroll from the first chapter, test on the first 10 utterances of the second chapter (a channel change stand-in; reported, not gating). Speakers: all speakers of the split (40 / 40 / 33). One scene per utterance (up to 15 s, 800 ms lead, 2 s tail), VAD as in the other benches.

**Causes (counterfactual, first that recovers the utterance).** 1 oracle reference (mean of unit embeddings of up to about 30 s of other utterances of the same chapter, not enrolled and not tested, same bed condition); 2 bed removed from the test utterance only (bed rows); 3 short utterance (at most 2 windows); 4 speaker the model handles badly (the speaker's window p10 against the oracle reference is below the threshold); 5 unexplained. A recovered utterance is one that ends `caller` with no lost word.

**Run.** Models from the primary checkout `examples/voice-agent-local-sherpa/.models`, LibriSpeech under `/tmp/librispeech/LibriSpeech` (`dev-other` from openslr SLR12, CC BY 4.0, converted to 16 kHz wav like the other sets), bed `/tmp/sg-demo/bed/tocumen48.f32`. Audio never committed.

```bash
cargo build --release -p node-webrtc-rust-vendor-sherpa-onnx --example near_miss_bench
development/scripts/host-safety/heavy-run.sh --label ul-<set> -- target/release/examples/near_miss_bench --utterance-loss \
  --model $M/speaker/nemo_en_titanet_small.onnx --stt $M/sherpa-onnx-streaming-zipformer-en-2023-06-26 \
  --librispeech /tmp/librispeech/LibriSpeech --ls-split <test-clean|dev-clean|dev-other> \
  --bed /tmp/sg-demo/bed/tocumen48.f32 --out /tmp/ul-<set>.md
```

**Parity.** `NM_SPLIT_DUMP` on the dev-clean bed + clean run dumped 92 product other-to-caller confirms; `cargo test -p node-webrtc-rust-speech bench_split_parity -- --ignored` replays them through the product `other_to_caller_split` + `scan_caller_sub_window`: 92 same, 0 differ (90 with a caller sub-window, 2 deferred). The caller -> other cut uses the product function directly, so it has no second parity check. The test needed one mechanical fix: `ps - pre` overflowed for a first window starting before 300 ms (`saturating_sub`, as the bench's `slice` does). Bench commit of the runs: e1d7608c (first line of each log; the parity dump is from the dev-clean run of that commit: 92 same, 0 differ).

**Limits.** One VAD utterance per file scene (a file the VAD splits yields several utterances, each judged on its own); files are cut at 15 s, so an utterance has at most 10 windows and windows 11 and 12 of the 12-window re-check are never reached; a speaker is skipped for a mode when the oracle pool is empty or the enrollment cannot be built; the bystander sample is about 1,500 windows per row; the Piper reroll, the `false_accept_bench`-scale per-window FAR, long-other and airport A/B are not part of this chunk.

**Limits.** One VAD utterance per file scene (a file the VAD splits yields several utterances, each judged on its own); files are truncated at 15 s, possibly mid-word, so an utterance has at most 10 windows and windows 11 and 12 of the 12-window re-check are never reached; bed rows carry a 2 s bed-only tail (`TAIL_MS`) after the speech; a speaker is skipped for a mode when the oracle pool is empty or the enrollment cannot be built; the bystander sample is about 1,500 windows per row; the Piper reroll, the `false_accept_bench`-scale per-window FAR, long-other and airport A/B are not part of this chunk.

### Result (utterance outcome, words lost above the floor, causes, speakers)

Columns: utterances >= 1.5 s, unknown (< 1.5 s), dropped, partly lost, caller->other splits (total, then by the class of the confirming window: (a) bed-tail window, (b) post-truncation audio, (c) mid-speech, each as splits / caller words cut), lost utterances % (dropped + partly), words lost above the floor (all classes), words lost per speaker (median / max over the speakers of the row, speakers with no loss count as 0), cause counts over lost utterances, top 5 speakers.

| set | mode | utterances (>= 1.5 s) | unknown (< 1.5 s) | dropped | partly lost | caller->other splits: total | (a) confirmed by bed-tail window: splits / words | (b) confirmed by post-truncation audio: splits / words | (c) confirmed mid-speech: splits / words | lost utterances % | words lost above floor | words lost per speaker: median / max | bad enrollment | noise | short utterance | speaker the model handles badly | unexplained | top 5 speakers (words lost, utterances lost) | top 5 share of words |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LibriSpeech test-clean, Tocumen bed -12 dB | same-session | 557 | 21 | 15 | 9 | 10 | 6 / 18 | 0 / 0 | 4 / 63 | 4.3 | 189 | 0.0 / 66 | 13 | 3 | 4 | 4 | 0 | 8555: 66, 5; 672: 65, 6; 3729: 16, 2; 5639: 13, 2; 1995: 9, 2 | 89 % |
| LibriSpeech test-clean, Tocumen bed -12 dB | cross-session | 306 | 16 | 15 | 10 | 10 | 7 / 20 | 0 / 0 | 3 / 68 | 8.2 | 302 | 0.0 / 177 | 24 | 0 | 0 | 0 | 1 | 4446: 177, 10; 4992: 42, 3; 908: 28, 1; 7021: 14, 1; 2961: 10, 1 | 90 % |
| LibriSpeech test-clean, clean | same-session | 550 | 0 | 10 | 3 | 3 | 1 / 13 | 0 / 0 | 2 / 16 | 2.4 | 118 | 0.0 / 58 | 11 | 0 | 1 | 0 | 1 | 672: 58, 5; 3729: 21, 3; 1284: 17, 2; 5639: 13, 2; 8555: 9, 1 | 100 % |
| LibriSpeech test-clean, clean | cross-session | 300 | 0 | 10 | 1 | 1 | 1 / 7 | 0 / 0 | 0 / 0 | 3.7 | 184 | 0.0 / 177 | 11 | 0 | 0 | 0 | 0 | 4446: 177, 10; 4970: 7, 1 | 100 % |
| LibriSpeech dev-clean, Tocumen bed -12 dB | same-session | 550 | 24 | 12 | 10 | 9 | 5 / 24 | 0 / 0 | 4 / 90 | 4.0 | 204 | 0.0 / 102 | 16 | 1 | 3 | 1 | 1 | 6319: 102, 7; 3576: 63, 6; 3752: 18, 3; 1462: 9, 1; 84: 5, 1 | 97 % |
| LibriSpeech dev-clean, Tocumen bed -12 dB | cross-session | 304 | 20 | 17 | 13 | 13 | 9 / 44 | 0 / 0 | 4 / 69 | 9.9 | 513 | 0.0 / 292 | 28 | 1 | 0 | 1 | 0 | 1988: 292, 11; 6319: 83, 4; 8842: 55, 3; 7850: 36, 4; 84: 17, 1 | 94 % |
| LibriSpeech dev-clean, clean | same-session | 538 | 0 | 3 | 6 | 5 | 1 / 7 | 0 / 0 | 4 / 103 | 1.7 | 144 | 0.0 / 91 | 9 | 0 | 0 | 0 | 0 | 6319: 91, 5; 3576: 53, 4 | 100 % |
| LibriSpeech dev-clean, clean | cross-session | 300 | 0 | 22 | 3 | 3 | 0 / 0 | 0 / 0 | 3 / 42 | 8.3 | 494 | 0.0 / 295 | 25 | 0 | 0 | 0 | 0 | 1988: 295, 12; 6319: 126, 7; 8842: 50, 3; 7850: 16, 2; 6295: 7, 1 | 100 % |
| LibriSpeech dev-other, Tocumen bed -12 dB | same-session | 391 | 16 | 3 | 4 | 4 | 4 / 19 | 0 / 0 | 0 / 0 | 1.8 | 27 | 0.0 / 14 | 5 | 1 | 1 | 0 | 0 | 1686: 14, 3; 3660: 5, 1; 4515: 5, 1; 1651: 3, 2 | 100 % |
| LibriSpeech dev-other, Tocumen bed -12 dB | cross-session | 280 | 13 | 34 | 20 | 21 | 13 / 68 | 0 / 0 | 8 / 116 | 19.3 | 618 | 0.0 / 152 | 48 | 1 | 3 | 2 | 0 | 3915: 152, 7; 1585: 137, 9; 4570: 88, 7; 6455: 63, 7; 4153: 55, 8 | 80 % |
| LibriSpeech dev-other, clean | same-session | 387 | 0 | 2 | 0 | 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0.5 | 1 | 0.0 / 1 | 0 | 0 | 2 | 0 | 0 | 1686: 1, 1; 1651: 0, 1 | 100 % |
| LibriSpeech dev-other, clean | cross-session | 279 | 0 | 40 | 14 | 14 | 5 / 39 | 0 / 0 | 9 / 158 | 19.4 | 853 | 0.0 / 253 | 53 | 0 | 1 | 0 | 0 | 3915: 253, 10; 1585: 175, 11; 6455: 142, 10; 4831: 140, 7; 4570: 69, 6 | 91 % |

### Bystander baselines, same harness

Other speakers' utterances (3 bystanders per caller, 3 files each) scored against the same product-enrolled reference; accepted = window 1 `caller`, or an other-to-caller confirm, or the deferred veto pending at the end of speech.

| set | mode | windows | window FAR % | pairs | pairs >= 5 % | pairs >= 20 % | bystander-only utterances (>= 1.5 s) | accepted (window 1 caller, or confirm, or veto pending) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LibriSpeech test-clean, Tocumen bed -12 dB | same-session | 1642 | 0.00 | 102 | 0 | 0 | 333 | 2 |
| LibriSpeech test-clean, Tocumen bed -12 dB | cross-session | 1427 | 0.00 | 87 | 0 | 0 | 280 | 1 |
| LibriSpeech test-clean, clean | same-session | 1672 | 0.00 | 102 | 0 | 0 | 337 | 0 |
| LibriSpeech test-clean, clean | cross-session | 1450 | 0.00 | 87 | 0 | 0 | 284 | 0 |
| LibriSpeech dev-clean, Tocumen bed -12 dB | same-session | 1510 | 0.07 | 102 | 1 | 0 | 325 | 1 |
| LibriSpeech dev-clean, Tocumen bed -12 dB | cross-session | 1276 | 0.08 | 87 | 1 | 0 | 278 | 1 |
| LibriSpeech dev-clean, clean | same-session | 1547 | 0.00 | 102 | 0 | 0 | 323 | 2 |
| LibriSpeech dev-clean, clean | cross-session | 1312 | 0.00 | 87 | 0 | 0 | 275 | 2 |
| LibriSpeech dev-other, Tocumen bed -12 dB | same-session | 1134 | 0.97 | 75 | 1 | 1 | 243 | 4 |
| LibriSpeech dev-other, Tocumen bed -12 dB | cross-session | 1237 | 0.00 | 81 | 0 | 0 | 259 | 1 |
| LibriSpeech dev-other, clean | same-session | 1172 | 0.68 | 75 | 1 | 1 | 240 | 3 |
| LibriSpeech dev-other, clean | cross-session | 1277 | 0.00 | 81 | 0 | 0 | 258 | 0 |

### Reading the table

- Same-session (gating mode): 0.5 - 4.3 % of caller utterances are lost (words above the floor: test-clean 189 bed / 118 clean, dev-clean 204 / 144, dev-other 27 / 1). Cross-session: 3.7 - 19.4 % (184 - 853 words).
- Classed by the confirming window, class (b) is 0 splits / 0 words on all 12 rows: no cut is confirmed by audio after the 15 s truncation. Class (a), confirmed by a bed-tail window, is not 0: same-session test-clean 6 / 18 (bed), 1 / 13 (clean); dev-clean 5 / 24, 1 / 7; dev-other 4 / 19, 0 / 0; cross-session 7 / 20, 1 / 7, 9 / 44, 0 / 0, 13 / 68, 5 / 39. These are real product behaviour (the bed, or the trailing silence, goes on after the caller stops and confirms `other`), so their lost words count, but they are not cuts in the middle of speech. The previous version, which classed by the cut point, put all of them in (c): the cut lies at most at the middle of the first `other` window, so a cut confirmed by the bed tail still landed before the voiced end.
- Class (c), confirmed mid-speech (splits / words), same-session: test-clean 4 / 63 (bed), 2 / 16 (clean); dev-clean 4 / 90, 4 / 103; dev-other 0 / 0, 0 / 0. Share of the same-session lost words: (c) 33 %, 14 %, 44 %, 72 %, 0 %, 0 %; (a) 10 %, 11 %, 12 %, 5 %, 70 %, 0 % (dev-other clean has 1 lost word in total). So mid-speech caller->other cuts are a seventh to under three quarters of the same-session loss on test-clean and dev-clean and none on dev-other; whole-utterance drops are the rest (15 / 10, 12 / 3, 3 / 2 dropped utterances). Cross-session class (c): 3 / 68, 0 / 0, 4 / 69, 3 / 42, 8 / 116, 9 / 158, against 15, 10, 17, 22, 34 and 40 dropped utterances in the same order.
- The first version of this table (drops only, `partly lost` 0 - 1 per row) understated the loss; the second modelled a caller->other watch after an upgrade that the product does not run, which this version removes.
- The loss is concentrated in a few speakers: the median speaker loses 0 words in every row, the maximum is 1 - 102 words same-session and 152 - 295 cross-session, and the top 5 speakers hold 80 - 100 % of the lost words (672, 8555, 3729 in test-clean; 6319, 3576 in dev-clean; 1686 in dev-other same-session; 4446, 1988, 3915, 1585 cross-session).
- Cause: the oracle reference recovers most lost utterances. Same-session: 13 of 24 (test-clean bed), 11 of 13 (clean), 16 of 22 (dev-clean bed), 9 of 9 (clean), 5 of 7 (dev-other bed); cross-session 24 of 25, 11 of 11, 28 of 30, 25 of 25, 48 of 54, 53 of 54. Removing the bed from the test utterance recovers 0 - 3 per bed row, short utterances 0 - 4, the speaker-model cause 0 - 4, unexplained 0 - 1.
- Bystander accepts per set are 0 - 4 of about 240 - 340 utterances; the window FAR is 0.00 - 0.97 %, highest on dev-other same-session (one pair at or above 20 %).

## Candidate bench against the ship bar (caller-reject-rate, chunk 2a) - tuning set only, not a verdict

Chunk 2a: test-clean only, bed -12 dB and clean, 40 callers. Held-out sets (dev-clean, dev-other, Piper reroll), the step 0 baselines and the bar table are chunk 2b. Bench-only: `near_miss_bench --utterance-loss` takes `NM_CAND` (comma list; an unknown token is an error): `e<ms>` enrollment span, `c2` multi-segment enrollment (mean of unit embeddings per enrolled utterance), `c3` reject a poor enrollment (the enrollment's own 1.5 s sub-windows against the reference; minimum below `NM_C3_MIN` discards it, at most 3 tries, then the best try, also when the files run out), `c4n<N>` repeated near-miss accept, `c7r<N>` N consecutive `other` windows for a caller -> other cut, `c7nm` near-miss windows ([`NEAR_MISS_MIN_SCORE`, threshold)) are neutral for that run, `c7scan` the 750/125 caller sub-window scan over the confirming windows vetoes the cut and restarts the run. Logs `.test-logs/crr2a-<cand>-test-clean.log` carry the commit (aadf1815) and `NM_CAND` on line 1; the driver is one heavy-run (`/tmp/crr2a_run.sh`, 5 candidates in parallel).

Design notes (decisions of the parent, 2026-11-17):

- **Fixed test set across candidates.** Per chapter the test utterances are the last 15 files (cross-session: the last 10 files of the test chapter). Enrollment, for every candidate including today's rules, draws only from the files before them, in order; a speaker whose pre-test files cannot reach the span is **skipped** (5 speakers in every row, listed in the tables). The baseline therefore changed from chunk 1 (test-clean same-session bed 189 -> 194 words, clean 118 -> 91) and the first run's PARTIAL rows are replaced. The oracle reference pool is the 40 files just before the tests, the same for every candidate (it can overlap the enrollment files; it only attributes causes).
- **c7nm is neutral:** a near-miss window neither counts toward the `other` run nor resets it and does not become `prev` (unit test: [other, near-miss, other] with run 2 confirms on the third window; today's rules confirm on the second).
- **c7scan** scans every window of the confirming run with the product's 750/125 sub-windows and the verify-budget-free bench scan (optimistic: the product bounds it by `verify_timeout_ms`). A hit vetoes the cut, the run restarts and the scanned window becomes `prev`.
- **C4 accept does not arm the caller -> other watch** (as the product's upgrade arm). This is optimistic for C4: an accepted utterance gets no later cut.
- **C3 bounds** come from the enrollment self-score distribution (minimum 1.5 s sub-window cosine of the first enrollment of each speaker, all 128 enrollments of the test-clean enrollment-only run, bed and clean, same- and cross-session): min 0.385, p10 0.589, p25 0.683, median 0.764, p75 0.804, max 0.886. The bounds are p10 (0.589) and p25 (0.683): they reject about 10 % and 25 % of the enrollments. C3 at 0.35 (the first guess) would reject none.
- Words are measure A above the floor; `partly` and `dropped` are utterances.

Tuning set, test-clean, same-session (words lost above floor / dropped / partly; bystander-only utterances accepted of tested):

bed row, same-session:

| candidate | words lost | dropped | partly | words per speaker median / max | top 5 speakers (words lost, utterances lost) | bystander-only accepted | speakers skipped | enrollment ms median / p90 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| today (empty) | 194 | 12 | 14 | 0.0 / 71 | 1284: 71, 3; 672: 57, 7; 8555: 27, 3; 4507: 15, 4; 3729: 9, 1 | 2 of 342 | 5 | 9520 / 17120 |
| C2 | 167 | 11 | 12 | 0.0 / 71 | 1284: 71, 3; 672: 31, 5; 8555: 27, 3; 4507: 15, 4; 3729: 9, 1 | 2 of 342 | 5 | 9520 / 17120 |
| C3 bound 0.589 (p10) | 193 | 11 | 14 | 0.0 / 71 | 1284: 71, 3; 672: 56, 6; 8555: 27, 3; 4507: 15, 4; 3729: 9, 1 | 2 of 342 | 5 | 9560 / 21360 |
| C3 bound 0.683 (p25) | 193 | 11 | 14 | 0.0 / 71 | 1284: 71, 3; 672: 56, 6; 8555: 27, 3; 4507: 15, 4; 3729: 9, 1 | 2 of 342 | 5 | 10980 / 33260 |
| C2+C3 0.589 | 192 | 11 | 13 | 0.0 / 71 | 1284: 71, 3; 672: 56, 6; 8555: 27, 3; 4507: 15, 4; 3729: 9, 1 | 2 of 342 | 5 | 9560 / 21360 |
| C2+C3 0.683 | 187 | 11 | 12 | 0.0 / 71 | 1284: 71, 3; 672: 56, 6; 8555: 27, 3; 4507: 10, 3; 3729: 9, 1 | 2 of 342 | 5 | 10980 / 33260 |
| C4 N=2 | 134 | 8 | 14 | 0.0 / 50 | 1284: 50, 2; 672: 36, 6; 4507: 15, 4; 8555: 13, 2; 3729: 9, 1 | 6 of 342 | 5 | 9520 / 17120 |
| C4 N=3 | 138 | 9 | 14 | 0.0 / 50 | 1284: 50, 2; 672: 36, 6; 4507: 15, 4; 8555: 13, 2; 3729: 9, 1 | 2 of 342 | 5 | 9520 / 17120 |
| C7 run 3 | 144 | 12 | 2 | 0.0 / 71 | 1284: 71, 3; 672: 52, 6; 8555: 17, 2; 8455: 4, 1; 2961: 0, 2 | 2 of 342 | 5 | 9520 / 17120 |
| C7 near-miss neutral | 106 | 12 | 1 | 0.0 / 52 | 672: 52, 6; 1284: 33, 2; 8555: 17, 2; 8455: 4, 1; 2961: 0, 2 | 2 of 342 | 5 | 9520 / 17120 |
| C7 scan veto | 182 | 12 | 11 | 0.0 / 71 | 1284: 71, 3; 672: 52, 6; 8555: 27, 3; 4507: 12, 3; 3729: 9, 1 | 2 of 342 | 5 | 9520 / 17120 |
| C2+C7 run 3 | 123 | 11 | 2 | 0.0 / 71 | 1284: 71, 3; 672: 31, 5; 8555: 17, 2; 8455: 4, 1; 2961: 0, 2 | 2 of 342 | 5 | 9520 / 17120 |
| C2+C7 near-miss neutral | 85 | 11 | 1 | 0.0 / 33 | 1284: 33, 2; 672: 31, 5; 8555: 17, 2; 8455: 4, 1; 2961: 0, 2 | 2 of 342 | 5 | 9520 / 17120 |
| C2+C7 scan veto | 160 | 11 | 10 | 0.0 / 71 | 1284: 71, 3; 672: 31, 5; 8555: 27, 3; 4507: 12, 3; 3729: 9, 1 | 2 of 342 | 5 | 9520 / 17120 |

clean row, same-session:

| candidate | words lost | dropped | partly | words per speaker median / max | top 5 speakers (words lost, utterances lost) | bystander-only accepted | speakers skipped | enrollment ms median / p90 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| today (empty) | 91 | 6 | 2 | 0.0 / 42 | 1284: 42, 2; 672: 40, 4; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C2 | 91 | 6 | 2 | 0.0 / 42 | 1284: 42, 2; 672: 40, 4; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C3 bound 0.589 (p10) | 91 | 6 | 2 | 0.0 / 42 | 1284: 42, 2; 672: 40, 4; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9560 / 17440 |
| C3 bound 0.683 (p25) | 89 | 6 | 3 | 0.0 / 42 | 1284: 42, 2; 672: 38, 5; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9560 / 28400 |
| C2+C3 0.589 | 91 | 6 | 2 | 0.0 / 42 | 1284: 42, 2; 672: 40, 4; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9560 / 17440 |
| C2+C3 0.683 | 89 | 6 | 3 | 0.0 / 42 | 1284: 42, 2; 672: 38, 5; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9560 / 28400 |
| C4 N=2 | 79 | 5 | 2 | 0.0 / 40 | 672: 40, 4; 1284: 30, 1; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C4 N=3 | 91 | 6 | 2 | 0.0 / 42 | 1284: 42, 2; 672: 40, 4; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C7 run 3 | 61 | 6 | 1 | 0.0 / 40 | 672: 40, 4; 1284: 12, 1; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C7 near-miss neutral | 47 | 6 | 0 | 0.0 / 26 | 672: 26, 3; 1284: 12, 1; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C7 scan veto | 91 | 6 | 2 | 0.0 / 42 | 1284: 42, 2; 672: 40, 4; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C2+C7 run 3 | 61 | 6 | 1 | 0.0 / 40 | 672: 40, 4; 1284: 12, 1; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C2+C7 near-miss neutral | 47 | 6 | 0 | 0.0 / 26 | 672: 26, 3; 1284: 12, 1; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |
| C2+C7 scan veto | 91 | 6 | 2 | 0.0 / 42 | 1284: 42, 2; 672: 40, 4; 7127: 5, 1; 8455: 4, 1 | 0 of 346 | 5 | 9520 / 17120 |

Cross-session (reported, not gating), words lost (bed / clean): today (empty) 300 / 197; C2 283 / 197; C3 bound 0.589 (p10) 241 / 197; C3 bound 0.683 (p25) 241 / 193; C2+C3 0.589 228 / 197; C2+C3 0.683 228 / 193; C4 N=2 257 / 180; C4 N=3 257 / 180; C7 run 3 249 / 188; C7 near-miss neutral 189 / 131; C7 scan veto 290 / 184; C2+C7 run 3 236 / 188; C2+C7 near-miss neutral 176 / 131; C2+C7 scan veto 273 / 184.


C7 takeover rows (`near_miss_bench --second-cut --takeover-db -3`, LibriSpeech test-clean, shipped rule jj of the table; U2s = takeover scenes whose U1 splits; `caller -> other splits` = U2s whose takeover was cut; "bystander-2 words left" = bystander words still in the caller part). Missed takeover splits are baseline splits minus the candidate's, as a share of the U2s:

| candidate | bed: U2s | bed: splits | bed: missed vs today | bed: caller words cut (utts) | bed: bystander-2 words left / total | clean: U2s | clean: splits | clean: missed vs today | clean: caller words cut (utts) | clean: bystander-2 words left / total |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| today | 65 | 53 | 0 | 24 (18) | 97 / 492 | 93 | 79 | 0 | 57 (35) | 124 / 717 |
| C7 run 3 | 65 | 20 | 33 (51 %) | 12 (9) | 269 / 492 | 93 | 19 | 60 (65 %) | 28 (16) | 477 / 717 |
| C7 near-miss neutral | 65 | 42 | 11 (17 %) | 6 (5) | 211 / 492 | 93 | 58 | 21 (23 %) | 19 (11) | 310 / 717 |
| C7 scan veto | 65 | 26 | 27 (42 %) | 4 (3) | 270 / 492 | 93 | 30 | 49 (53 %) | 10 (7) | 418 / 717 |

Reading (tuning only):

- **Enrollment.** C2 helps the bed row (194 -> 167 words, 672 loses 57 -> 31) and leaves clean level (91). C3 does almost nothing at either bound (bed 193, clean 91 / 89): the poorly enrolled speakers do not have a low enrollment minimum, so the check does not find them. C3 adds fail-open time (median 9.5 -> 11.0 s and p90 17 -> 33 s at 0.683), so it is not worth it on this evidence. C2+C3 is no better than C2 alone (187 / 89 at 0.683).
- **C4.** N=2 lowers the bed row to 134 words and clean to 79, but raises bed bystander-only accepts from 2 to 6 of 342 (fails the keep rule). N=3 lowers bed to 138 and leaves clean level (91), with no rise in bystander accepts (2 of 342; 0 of 346). The same-session `dropped` count goes 12 -> 9 (bed). C4 is benched with the watch unarmed (optimistic).
- **C7.** Every C7 variant cuts the caller words lost in the loss rows by a lot (bed 194 -> 106 for near-miss neutral, 144 for run 3, 182 for scan; clean 91 -> 47, 61, 91) but the takeover rows show what that costs: 17 - 65 % of the takeovers that today split are no longer split, and 311 - 477 of 717 bystander words stay in the caller final instead of 124. All three exceed the +5 % missed takeovers bound by a wide margin; the smallest miss is near-miss neutral at 17 % (bed) and 23 % (clean). C7 x C2 does not change that (C2 does not touch the caller -> other watch).
- Bystander-only accepts do not change for any candidate except C4 N=2 (bed 2 -> 6 of 342).

Keep rule (test-clean words lost down in the bed or the clean row and at least level in the other, no rise in bystander-only accepts, and for C7 at most +5 % missed takeovers) and the 2b input:

| candidate | kept | why |
| --- | --- | --- |
| C2 | yes | bed 194 -> 167, clean 91 -> 91, accepts level |
| C4 N=3 | yes | bed 194 -> 138, clean 91 -> 91, accepts level; reported on the `dropped` subset in 2b; watch-unarmed optimism noted |
| C3 0.589 / 0.683 | marginal (1-2 words, +1.5 s median fail-open at 0.683) | not recommended for 2b: effect is within noise |
| C2+C3 | no benefit over C2 | not recommended |
| C4 N=2 | no | bed bystander accepts 2 -> 6 |
| C7 run 3, near-miss neutral, scan veto (alone and with C2) | no | missed takeovers 17 - 65 % against a 5 % bound |

Not run in 2a: C2 + C4 N=3 together (the parent can add it in 2b), C1 8000, every held-out set, the `false_accept_bench`-scale baselines, long-other 12/18 s, airport A/B, the 375-takeover `caller_to_other_bench` set. The takeover rows above are the librispeech takeover scenes of `near_miss_bench`, not that set.

Common Voice: not used. The official dataset page (commonvoice.mozilla.org/en/datasets) is script-rendered and shows no licence text to a plain fetch; the only CC0 statement reachable is the project README on GitHub, which is not the dataset page, and the download needs an account and terms acceptance. Per decision oo it is skipped.

### Piper reroll, seed 20261117 (caller-reject-rate, chunk 2b-1)

`speaker_fixtures --seed 20261117`, no-pause set (n, 400 scenes) and paused set (p, 200), same flags as the published sets. Window scores by `near_miss_bench --window-dist` (TitaNet-small, threshold 0.459; the 1.5 s windows of the merged and bystander-only utterances, the windows the rules act on; log `.test-logs/20261117-105800-crr2b1-window-dist.log`). The reroll exercises the same rule bands as the published sets:

| set | windows | p10 | p50 | p90 | share < 0.20 | share in [0.30, thr) |
| --- | --- | --- | --- | --- | --- | --- |
| reroll n (400) | 1913 | 0.054 | 0.355 | 0.780 | 33.3 % | 11.9 % |
| published n400 | 1976 | 0.043 | 0.358 | 0.783 | 34.2 % | 12.2 % |
| reroll p (200) | 635 | 0.010 | 0.134 | 0.351 | 67.7 % | 12.9 % |
| published p200 | 628 | 0.004 | 0.109 | 0.360 | 67.7 % | 12.4 % |

### Bar 6, long-other 12 / 18 s per candidate (caller-reject-rate, chunk 2b-1, decision vv)

`speaker_bench --long-other` at 005878bf on the regenerated default-seed fixtures (`speaker_fixtures` default seed, manifest sha256 `da4068d2...`), TitaNet-small, threshold 0.459, check 1.5 s, pooled tune + test, 480 streams per cell. Candidate switches: `LO_ENROLL=product` (whole enrollment segments until the `NM_CAND` span) with `NM_CAND` unset, `c2`, `c4n3`, `c2,c4n3`. C4 applies to the consecutive scheme only (the sliding column is unchanged by construction).

Determinism (vv): two default runs (`LO_ENROLL` and `NM_CAND` unset), one at 1a0a0e19 and one at 005878bf, give byte-identical `long_other.md` tables. The only differing lines are the `git` SHA in the header, the "One embed ... p50 / p99" line and the cost table D (CPU s at p50, derived from that timing). Default path: 3.96 % mixed / 9.38 % one voice at 18 s (the published 3.33 % / 10.42 % do not reproduce, see vv).

Pooled consecutive FAR (accepted / 480) at T = 12 s and 18 s. Baseline = `LO_ENROLL=product`, `NM_CAND` empty.

| candidate | mixed 12 s | mixed 18 s | one voice 12 s | one voice 18 s |
| --- | --- | --- | --- | --- |
| default enrollment (segments concatenated), today | 2.50 % (12) | 3.96 % (19) | 6.04 % (29) | 9.38 % (45) |
| product enrollment, today (baseline) | 3.12 % (15) | 5.62 % (27) | 6.04 % (29) | 8.54 % (41) |
| product + C2 | 3.12 % (15) | 5.62 % (27) | 6.04 % (29) | 8.54 % (41) |
| product + C4 N=3 | 3.12 % (15) | 5.62 % (27) | 17.92 % (86) | 19.17 % (92) |
| product + C2 + C4 N=3 | 3.12 % (15) | 5.62 % (27) | 17.92 % (86) | 19.17 % (92) |

Reading: C2 is identical to the baseline in every cell (the product enrollment takes one segment: every reference scene's first enrollment segment is ≥ 4992 ms ≥ the 3000 ms span, so the C2 mean is that one embedding, identical by construction; bar 6 does not exercise C2 here). C4 N=3 leaves the mixed set unchanged and raises the one-voice consecutive FAR from 6.04 % / 8.54 % to 17.92 % / 19.17 %: a same-voice stream has many near-miss windows, so a repeated near-miss accept admits it. The sliding column is the same for all four candidates (6.04 / 9.79 mixed, 10.42 / 12.08 one voice at 12 / 18 s). C4 N=3 fails bar 6 (+10.6 pt one voice at 18 s against +0.5 pt; plan decision ww).

### Bar 1 gate (decision ww) (caller-reject-rate, chunk 2b-2)

Built at 0c923017. `near_miss_bench --utterance-loss`, today (NM_CAND "") vs C2 (NM_CAND `c2`), dev-clean and dev-other, Tocumen bed -12 dB and clean. Logs: `.test-logs/20261117-121352-crr2b2-gate-ul-{today,c2}-{dev-clean,dev-other}.log` (SHA and NM_CAND on line 1); outputs `/tmp/crr2b2g-ul-*.md`.

Caller words lost above the floor, same-session (the binding enrollment, §5):

| set | row | today | C2 | today ≥ 10 | mm verdict |
| --- | --- | --- | --- | --- | --- |
| dev-clean | bed | 241 | 237 | yes | pass (lower) |
| dev-clean | clean | 199 | 199 | yes | **fail (equal, not strictly lower)** |
| dev-other | bed | 60 | 52 | yes | pass (lower) |
| dev-other | clean | 0 | 0 | no | pass (not higher) |

Cross-session, reported only: dev-clean bed 567 / 521, clean 544 / 539; dev-other bed 485 / 480, clean 550 / 550.

Bar 2 (dropped), same-session today / C2: dev-clean bed 13 / 13, clean 10 / 10; dev-other bed 1 / 1, clean 0 / 0. Cross-session: dev-clean bed 26 / 27 (+1), clean 30 / 30; dev-other bed 32 / 31, clean 33 / 32.
Bar 3 (caller-only false splits), same-session: dev-clean bed 12 / 12, clean 5 / 5; dev-other bed 14 / 11, clean 0 / 0. Cross-session: dev-clean bed 16 / 11, clean 4 / 3; dev-other bed 9 / 10 (+1), clean 7 / 8 (+1).

Piper reroll, today only: the run was stopped by decision before it finished and is not reported. C2 cannot change the Piper sets: every scene has exactly one enrollment segment, 5457-5523 ms in both reroll fixtures, which is at least the 3000 ms span, so the C2 mean is that one embedding (equal by construction).

**Verdict: C2 fails bar 1 on dev-clean clean (199 -> 199). No candidate passes; the plan stops per §5.** (C4 N=3 had already failed bar 6, decision ww.)

### Open harness defect: `--evidence` default path differs between c134666a and 1a0a0e19 (not fixed)

`crr2b1-ev-ident` (pre c134666a vs post 1a0a0e19, NM_CAND default, log `.test-logs/20261117-110615-crr2b1-ev-ident.log`) is not identical beyond timing lines. The last column of the Piper straddle and LibriSpeech straddle rows differs, e.g. Piper straddle today `433 / 6450 / 52539 / 162` vs `433 / 6450 / 51627 / 157`. The `no_confirm` refactor in 1a0a0e19 changed a default-path count; the cause has not been investigated. Nothing in the bar-1 verdict depends on `--evidence`.

## zh_en and C7 per-utterance bar: bench switches and constant mapping (plan `2026-11-18-caller-loss-zhen-c7`, chunk 1)

Bench-only (no product code). `near_miss_bench` takes the gate's score constants as flags, with today's TitaNet-small values as defaults, so a run without them is byte-identical: `--thr` (alias of `--threshold`), `--nm-floor`, `--confident-other`, `--adapt-margin`, `--anchor-min`; `--adapt` runs the in-call reference refresh in `--utterance-loss` (the test utterances of one speaker and mode in order; an utterance commits only when every window scored `caller` and one scored at least `thr + margin`; EMA as `speaker_gate::adapt_step`, anchor bound as a flag; bystander baselines then use the adapted reference); `false_accept_bench --map-constants <dir>:<file>` prints the mapping below. The constants reach the near-miss band in `product_o2c`, the C7 near-miss neutral switch, decision bb (U1 window below the confident-other bound), and C4. The long-other path in `speaker_bench` still uses `cand::c4_scores` with the product constants (C4 is not a row of this plan).

**R0 parity (default flags, test-clean, bed -12 dB and clean, `--ls-split test-clean`).** Log `.test-logs/20261118-215810-r0-test-clean.log`, built at 077cec71 plus this chunk's bench edits. Same-session words lost above floor: bed 194 (12 dropped, 14 partly, top five `1284: 71, 3; 672: 57, 7; 8555: 27, 3; 4507: 15, 4; 3729: 9, 1`, bystander-only accepted 2 of 342), clean 91. These are the predecessor's "today (empty)" row, exactly. Cross-session: bed 300, clean 197.

**Constant mapping, zh_en (`3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced`, sha256 `aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2`).** Plan §8 decision a. The first mapping in this chunk (a `near_miss_bench --map-constants` pool of about 4.4k impostor windows per condition) is **replaced** and that mode is removed: TitaNet-small had no impostor window at or above 0.459 in the bed row or at or above 0.559 anywhere, so the survival match was degenerate (threshold 0.5451, margin 0). The mapping now runs on the `false_accept_bench` same-session pool, the one that gave the screen's 0.457 / 0.468:

```text
FA_ENROLL=product heavy-run.sh --label fa-map-constants -- target/release/examples/false_accept_bench \
  --voices-root $M --speaker-dir /tmp/speaker-models --bed /tmp/sms-inputs/bed/tocumen48.f32 --cache /tmp/sms-inputs/cache \
  --airport-a ... --airport-b ... --bed-db -12 --librispeech /tmp/librispeech/LibriSpeech \
  --map-constants /tmp/speaker-models:3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx
```

Log `.test-logs/20261118-223401-fa-map-constants.log`, exit 0. Test-clean, 40 speakers, same-session, product enrollment, 1.5 s windows, bed -12 dB and clean: 111,969 impostor windows per condition, 5,742 genuine windows (bed + clean). Impostor-side constants are FAR-matched per condition with `far_matched` (lowest score with at most the TitaNet survivor count at or above it) and the higher of bed and clean is taken. A constant with zero TitaNet survivors in a condition stops the run; none did.

| constant | TitaNet value | TitaNet survivors (bed / clean) | mapped bed | mapped clean | mapped zh_en value |
| --- | --- | --- | --- | --- | --- |
| threshold | 0.459 | 175 / 84 | 0.4574 | 0.4679 | **0.4679** |
| near-miss floor | 0.300 | 3169 / 2659 | 0.3119 | 0.3116 | **0.3119** |
| confident-other | 0.200 | 14221 / 11883 | 0.2128 | 0.2163 | **0.2163** |
| threshold + margin (genuine survival, pooled) | 0.559 | 5312 of 5742 genuine | 0.5935 | | 0.5935 |
| anchor bound (genuine quantile, pooled) | 0.800 | 682 of 5742 genuine | 0.8048 | | **0.8048** |

Mapped margin: 0.5935 - 0.4679 = **0.1256** (not floored). Flags for the zh_en rows: `--thr 0.4679 --nm-floor 0.3119 --confident-other 0.2163 --adapt-margin 0.1256 --anchor-min 0.8048`.

**Parity (constant plumbing).** `NM_SPLIT_DUMP` on a full dev-clean bed + clean run (40 callers, default flags; `.test-logs/20261118-223843-split-dump-full.log`, exit 0) dumped 128 other-to-caller confirms; `cargo test -p node-webrtc-rust-speech bench_split_parity -- --ignored` (`.test-logs/20261118-225537-split-parity-full.log`): 128 same, 0 differ (120 with a caller sub-window, 8 deferred). The earlier 12-caller dump compared only 17.

## zh_en and C7 per-utterance bar: result (plan `2026-11-18-caller-loss-zhen-c7`, chunk 2)

Bench-only. Binaries built at f37b3d69 (`/tmp/cl2/bin`); one queue, one heavy-run at a time, every run exit 0 (`/tmp/cl2/status.txt`, logs `.test-logs/20261118-*` and `20261119-*-cl2-<label>.log`, sha and `NM_CAND` on line 1, tables in `/tmp/cl2/*.md`). Exception: the Piper reroll `--evidence` rows (`/tmp/cl2/ev2-R{0,1,2,3}.md`, queue `/tmp/cl2/q2.sh`, logs `.test-logs/20261119-*-cl2b-ev2-R*.log`) use `/tmp/cl2/bin2`, built from the tree later committed as ae562c16 (logs read `9d2c1e69(+wip)`; no source newer than the binary, and ae562c16..HEAD has no code change). Inputs checked before every run (airport A/B, zh_en model, bed `/tmp/sms-inputs/bed/tocumen48.f32`, LibriSpeech trees, Piper reroll and bar-6 fixtures `da4068d2`); none changed. Rows: R0 TitaNet-small today; R1 zh_en, mapped constants (`--thr 0.4679 --nm-floor 0.3119 --confident-other 0.2163 --adapt-margin 0.1256 --anchor-min 0.8048`); R2 / R3 = R0 / R1 with `NM_CAND=c7nm` (C7 near-miss neutral); R4 / R5 = R0 / R1 with `--adapt`. Same-session, product enrollment, Tocumen bed -12 dB and clean. R0 reproduces the predecessor's "today" row (dev-clean 241 / 199 words, dev-other 60 / 0, dropped 13 / 10 / 1 / 0).

Commands (per row; `$ROW` = the flags above):

```text
near_miss_bench --utterance-loss --model <titanet|zh_en> --stt $STT --librispeech $LS --bed $BED --ls-split <dev-clean|dev-other> $ROW      # bars 1, 2, 3, 5 and the bystander window FAR
near_miss_bench --second-cut --takeover-db -3 ... --ls-split <dev-clean|dev-other>                                                         # missed takeovers, R0-R3
near_miss_bench --evidence --fixtures /tmp/crr2b-reroll-n --fixtures /tmp/crr2b-reroll-p --straddle-piper ... (same two dirs)             # Piper reroll, R0 R1; second queue /tmp/cl2/q2.sh also R2 (NM_CAND=c7nm) and R3
LO_ENROLL=product speaker_bench --long-other --fixed-threshold <0.459|0.4679> --checks 1.5 [--long-other-same-voice]                      # bar 6
FA_ENROLL=product FA_FIXED_THR=0.4679 false_accept_bench --models /tmp/speaker-models:<zh_en>.onnx                                         # bar 4 at scale, S4 margin
```

### Words lost above the floor / dropped / caller-only false splits (caller -> other splits), same-session

Cell = words / dropped utterances / splits. Binding value for bars 1, 2, 3.

| row | dev-clean bed | dev-clean clean | dev-other bed | dev-other clean | Piper reroll (merged no-pause, deferred veto (hh): lost words incl. dropped utterances / dropped utterances; straddle 0 / 0 in every row) |
| --- | --- | --- | --- | --- | --- |
| R0 | 241 / 13 / 12 | 199 / 10 / 5 | 60 / 1 / 14 | 0 / 0 / 0 | 12 / 0 |
| R1 | 125 / 9 / 10 | 151 / 9 / 8 | 35 / 5 / 7 | 9 / 3 / 0 | 66 / 3 (60 of the words) |
| R2 | 129 / 13 / 1 | 120 / 10 / 0 | 4 / 1 / 2 | 0 / 0 / 0 | 12 / 0 (= R0) |
| R3 | 55 / 9 / 0 | 70 / 9 / 0 | 13 / 5 / 1 | 9 / 3 / 0 | 66 / 3 (= R1) |
| R4 | 167 / 11 / 6 | 165 / 8 / 3 | 9 / 1 / 5 | 0 / 0 / 0 | not measured (adapt is a no-op in `--evidence`: output equals R0) |
| R5 | 93 / 6 / 8 | 123 / 5 / 7 | 15 / 2 / 4 | 0 / 0 / 0 | not measured (equals R1) |

### Bystanders, same harness (same-session; window FAR % / pairs >= 5 % / pairs >= 20 % / bystander-only utterances accepted of total)

Sample of about 1.3-1.6k windows per cell (3 bystanders per caller), so one window is 0.06-0.08 pt. This is not the 118k-window bar-4 measure.

| row | dev-clean bed | dev-clean clean | dev-other bed | dev-other clean |
| --- | --- | --- | --- | --- |
| R0 | 0.06 / 1 / 0 / 1 of 336 | 0.00 / 0 / 0 / 2 of 334 | 0.86 / 1 / 1 / 4 of 271 | 0.61 / 1 / 1 / 3 of 268 |
| R1 | 0.00 / 0 / 0 / 1 | 0.00 / 0 / 0 / 1 | 0.00 / 0 / 0 / 2 | 0.08 / 1 / 0 / 3 |
| R2 | = R0 (C7 does not touch it) | = R0 | = R0 | = R0 |
| R3 | = R1 | = R1 | = R1 | = R1 |
| R4 | 0.06 / 1 / 0 / **3** | 0.00 / 0 / 0 / 3 | **0.94** / 1 / 1 / **6** | **0.83** / 1 / 1 / 3 |
| R5 | 0.06 / 1 / 0 / **3** | 0.00 / 0 / 0 / 3 | 0.08 / 1 / 0 / 2 | 0.08 / 1 / 0 / 3 |

### Missed takeovers (`--second-cut --takeover-db -3`, rule jj, shipped; splits / U2s; missed = baseline splits minus the row's, over the U2s)

Baseline for the C7 effect is the same model without C7 (R2 vs R0, R3 vs R1). R4 / R5: not measured, the second-cut path has no adapt hook. The `bystander-2 words left` column is the bystander words still in the caller final.

| set | cond | R0 splits / U2s | R2 splits | R2 missed vs R0 | R2 bystander-2 left / total | R1 splits / U2s | R3 splits | R3 missed vs R1 | R3 bystander-2 left / total |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| dev-clean | bed | 44 / 57 | 34 | 10 (17.5 %) | 168 / 449 (R0 90) | 49 / 60 | 42 | 7 (11.7 %) | 151 / 472 (R1 84) |
| dev-clean | clean | 70 / 91 | 49 | 21 (23.1 %) | 294 / 733 (R0 153) | 78 / 93 | 60 | 18 (19.4 %) | 254 / 752 (R1 112) |
| dev-other | bed | 47 / 55 | 37 | 10 (18.2 %) | 117 / 425 (R0 55) | 43 / 51 | 38 | 5 (9.8 %) | 78 / 390 (R1 42) |
| dev-other | clean | 71 / 83 | 50 | 21 (25.3 %) | 179 / 631 (R0 88) | 62 / 74 | 50 | 12 (16.2 %) | 138 / 567 (R1 88) |

### C7 trade: caller words saved against takeovers missed (words lost, same-session)

| set | cond | R0 -> R2 words | R2 missed takeovers | R1 -> R3 words | R3 missed takeovers |
| --- | --- | --- | --- | --- | --- |
| dev-clean | bed | 241 -> 129 (-112) | 10 of 57 (17.5 %) | 125 -> 55 (-70) | 7 of 60 (11.7 %) |
| dev-clean | clean | 199 -> 120 (-79) | 21 of 91 (23.1 %) | 151 -> 70 (-81) | 18 of 93 (19.4 %) |
| dev-other | bed | 60 -> 4 (-56) | 10 of 55 (18.2 %) | 35 -> 13 (-22) | 5 of 51 (9.8 %) |
| dev-other | clean | 0 -> 0 (0) | 21 of 83 (25.3 %) | 9 -> 9 (0) | 12 of 74 (16.2 %) |

C7 also leaves more bystander speech in the caller final: R0 -> R2 roughly doubles the bystander-2 words left (90 -> 168, 153 -> 294, 55 -> 117, 88 -> 179 on dev-clean bed / clean, dev-other bed / clean). No gating bar covers this; bar 7 reports leaks on the Piper merged set only, and the takeover bound is the only guard.

R3 against R0 (zh_en + C7 against today): words 241 -> 55, 199 -> 70, 60 -> 13, 0 -> 9; split rate 77 % / 77 % / 85 % / 86 % (R0) against 70 % / 65 % / 75 % / 68 % (R3).

### Bar 6, long-other at 18 s (`speaker_bench --long-other`, `LO_ENROLL=product`, regenerated fixtures `da4068d2`, pooled 480 streams, consecutive scheme)

| row | threshold | mixed 18 s | one voice 18 s |
| --- | --- | --- | --- |
| R0 | 0.459 | 5.62 % (27) | 8.54 % (41) |
| R1 | 0.4679 | **8.33 % (40), +2.71 pt** | 7.71 % (37), -0.83 pt |
| R2 | = R0 by construction (C7 acts on the caller -> other watch, the long-other rule depends on the threshold only) | | |
| R3 | = R1 by construction | | |
| R4, R5 | not measured: `speaker_bench` has no in-call reference refresh in the long-other path | | |

R0 reproduces the predecessor's baseline (5.62 % / 8.54 %).

### Bar 4 at scale for R1, bar 8 input, bar 9

- **Bar 4 (R1, `false_accept_bench`, 118k windows per cell, FA_FIXED_THR=0.4679):** FAR delta -0.020 / +0.002 / -0.003 / +0.001 pt (within +0.03), pairs >= 5 % 6 / 4 / 3 / 5 against 7 / 5 / 2 / 3 (dev-other clean +2, bound +1), **new pair >= 20 % on dev-other bed and clean**. Fails (new pair, pairs). Rows in `FALSE_ACCEPT_BENCH.md`, "zh_en at the mapped threshold 0.4679". R2 = R0 and R3 = R1 (C7 does not change scores); R4 / R5 not measured (`false_accept_bench` has no adapt).
- **S4 airport margin, not bar 8** (decision b; reproduced offline at the row's own threshold, minimum of score - thr over the caller windows of scenes A and B): TitaNet-small at 0.459 +0.086 (this scene build), zh_en at 0.4679 +0.104. R2 = R0 and R3 = R1 (C7 does not change scores); R4 / R5 not measured (no adapt in `false_accept_bench`). Bar 8 itself is not judged: the replay harness (chunk 2b) is built only for a row that passes bars 1-7 and 9.
- **Bar 9:** C7 and adapt add no time before the first verdict (they act after it); the zh_en rows use the screen's Model check: 1.5 s embed p99 27.3 ms (TitaNet-small 19.2 ms, bound 38.4) and the 19-window scan p99 261 ms (bound 450 ms). All rows pass.

### Verdicts against bars 1-7 and 9 (same-session, held-out)

| row | bar 1 words | 2 dropped | 3 false splits | 4 per-window FAR / pairs | 5 bystander accepts | 6 long-other 18 s | 7 merged no-pause (reported) | 9 latency | passes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| R0 | reference | | | | | | leaked 522 / upgraded 27 | ok | |
| R1 | **fail**: dev-other clean 0 -> 9, Piper 12 -> 66 | **fail**: dev-other bed 1 -> 5, clean 0 -> 3, Piper merged 0 -> 3 | **fail**: dev-clean clean 5 -> 8 | **fail**: new pair >= 20 % on dev-other, pairs >= 5 % +2 | ok (1, 1, 2, 3 against 1, 2, 4, 3) | **fail**: mixed +2.71 pt | leaked 1086 / upgraded 60 | ok | no: 1, 2, 3, 4, 6 |
| R2 | **fail**: Piper 12 -> 12 (not strictly lower); LibriSpeech ok | ok (equal) | ok | ok (= R0) | ok (= R0) | ok (= R0) | = R0 | ok | no: 1 (Piper cell); LibriSpeech cells pass; missed takeovers 17.5-25.3 % (user bound) |
| R3 | **fail**: dev-other clean 0 -> 9, Piper 12 -> 66 | **fail**: as R1 | ok | **fail** (= R1) | ok (= R1) | **fail** (= R1) | = R1 | ok | no: 1, 2, 4, 6 |
| R4 | ok (Piper: not measured) | ok | ok | **fail** on the sample: dev-other FAR +0.08 / +0.22 pt | **fail**: dev-clean bed 1 -> 3, dev-other bed 4 -> 6 (+2) | not measured | = R0 | ok | no: 4, 5 |
| R5 | ok | **fail**: dev-other bed 1 -> 2, Piper merged 0 -> 3 | **fail**: dev-clean clean 5 -> 7 | **fail** at scale as R1 (sample ok) | **fail**: dev-clean bed 1 -> 3 | **fail** as R1 (adapt part not measured) | = R1 | ok | no: 2, 3, 4, 5, 6 |

Notes on the cells:

- Bar 5's second clause (at or below 0.5 % of the set) is already exceeded by R0 on dev-other (4 of 271); the verdict uses the +1 clause.
- Bar 1 on the Piper reroll (decision mm: caller words lost strictly lower on every held-out set that loses >= 10 words today, not higher elsewhere). The Piper column is the merged no-pause "deferred veto (hh)" row of the Scan-miss veto table (shipped mode 3, as `--utterance-loss`), `lost (A)` including the words of dropped utterances: R0 12 / 0, R1 66 / 3, R2 12 / 0, R3 66 / 3 (`/tmp/cl2/ev2-R{0,1,2,3}.md`; R1 66 = 6 split-lost + 3 dropped utterances of 60 words). `--evidence` now runs the same caller -> other watch as `--utterance-loss` (`ev_watch_lost`: `ul_c2o_whole` for a whole utterance, `u2_cut_span` with rule 1 for a split-born U2), counts the caller words it cuts above the floor, and honours `NM_CAND=c7nm`. The watch cuts 0 caller words on Piper: the 6j per-set table ("today: always split") is identical before and after the watch for R0-R3 (old `/tmp/cl2/ev-R{0,1}.md` against `ev2-R*.md`; only latency columns differ). The 6j table adds c2o (mode 3) to the splits from mode 0; that is harmless here because c2o = 0. Merged no-pause loses 12 words today (>= 10), so R2 must be strictly lower and is not: **R2 bar 1: Piper 12 -> 12, fail**. R2 fails bar 1 on the Piper reroll. Those 12 lost words are not caller -> other cuts, so C7 cannot act on them. **R1 / R3: Piper 12 -> 66 (3 dropped utterances, 60 words), fail.** R4 / R5: `--adapt` is still a no-op in `--evidence`, so those Piper cells are not measured.
- Bar 4 for R4 uses the 1.3-1.6k-window bystander sample; its +0.08 / +0.22 pt are one or three windows. The bar-5 failure (+2 accepted utterances) does not depend on that.
- Fixtures: every claim above scores its scenes with the row's own `--model` (`near_miss_bench` embeds every window with it; the Piper reroll is scored the same way in the `--evidence` rows). The rules are exercised: C7 rows cut caller -> other splits from 12 / 5 / 14 / 0 to 1 / 0 / 2 / 0 (R0 -> R2) and 10 / 8 / 7 / 0 to 0 / 0 / 1 / 0 (R1 -> R3); the zh_en rows change the drop and window counts through the mapped constants; adapt changes the words (R0 -> R4 241 -> 167, 199 -> 165, 60 -> 9) and the accepts.
- The chunk-1 build had the bench binaries built before the `FA_FIXED_THR` helper was factored into `model_threshold`; behaviour is identical (unit test `fixed_threshold_override_applies_to_every_non_reference_model`).
