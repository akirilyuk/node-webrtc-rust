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

## Caller false rejects: three bench rounds, no product change (2026-11-17 to 2026-11-19)

Goal: lose fewer caller words to the speaker gate (the caller's own voice scored below the threshold) without letting more bystander speech through. Every round was bench-only. The gate in `crates/speech` is unchanged, and the defaults stay as they are: `splitOnSpeakerChange` false, `adaptReference` false, TitaNet-small at 0.459. The bench modes used here (`near_miss_bench --utterance-loss`, `false_accept_bench` model screen and `--map-constants`) live on the unmerged branches `feature/caller-reject-rate` and `feature/speaker-model-screen`. The full tables are in this file and in `FALSE_ACCEPT_BENCH.md` on those branches.

**Setup.** Enrollment mirrors the product: whole caller utterances are appended until they reach 3000 ms, then one embed. Same-session mode enrolls on a speaker's first utterances and tests on the next 15. Tune set: LibriSpeech test-clean. Held-out sets: dev-clean and dev-other (OpenSLR SLR12, CC BY 4.0), plus rerolled Piper scenes. Every set runs with the Tocumen airport bed at -12 dB and clean. Words are counted with measure A (recognizer diff above the floor).

**Today's loss** (0.20.0 rules, same-session):

| set        | split off: utterances dropped (bed / clean) | split on: utterances lost, dropped or cut (bed / clean) |
| ---------- | ------------------------------------------- | ------------------------------------------------------- |
| dev-clean  | 12 of 550 / 3 of 538                        | 4.0 % / 1.7 %                                           |
| test-clean | 15 of 557 / 10 of 550                       | 4.3 % / 2.4 %                                           |

The loss is concentrated: 2 to 5 speakers hold 89-100 % of the lost words in each set. On the tune set, a better reference (the oracle mean of about 30 s of the same speaker) would recover most lost utterances. Removing the noise bed or lengthening the utterance recovers few.

**Round 1: gate rules** (mean-of-segments enrollment, rejecting a poor enrollment, accepting on repeated near-miss windows, a stricter caller→other confirm). None passes. Repeated near-miss accept lets bystanders through over long stretches (long-other 18 s, one voice: 8.54 → 19.17 %). Mean enrollment does not lower held-out loss (dev-clean clean 199 → 199 words).

**Round 2: other models** from the sherpa-onnx `speaker-recongition-models` release (13 assets checked for licence and load, 9 screened at matched pooled FAR). Only 3D-Speaker CAM++ zh_en advanced lowers caller rejects (worst cell -44 %). It adds bystander pairs: +4 pairs at or above 5 % FAR, and a new pair at or above 20 %. TitaNet-large has a 42 % higher worst-cell FRR than TitaNet-small. The VoxCeleb-trained English assets separate LibriSpeech speakers poorly at 1.5 s.

**Round 3: per-utterance bar for zh_en and for C7 (no caller→other cut on a near-miss window).** zh_en ran with constants mapped to TitaNet's (impostor-side constants FAR-matched on about 112k impostor windows: threshold 0.4679, near-miss floor 0.3119, confident-other 0.2163). Words lost / dropped utterances / caller-only false splits, bed then clean:

| row                              | dev-clean           | dev-other      | Piper reroll (words / drops) |
| -------------------------------- | ------------------- | -------------- | ---------------------------- |
| TitaNet-small, today             | 241/13/12, 199/10/5 | 60/1/14, 0/0/0 | 12 / 0                       |
| zh_en                            | 125/9/10, 151/9/8   | 35/5/7, 9/3/0  | 66 / 3                       |
| TitaNet-small + C7               | 129/13/1, 120/10/0  | 4/1/2, 0/0/0   | 12 / 0                       |
| zh_en + C7                       | 55/9/0, 70/9/0      | 13/5/1, 9/3/0  | 66 / 3                       |
| TitaNet-small + `adaptReference` | 167/11/6, 165/8/3   | 9/1/5, 0/0/0   | -                            |

- zh_en loses more words on Piper (12 → 66). It also raises long-other FAR at 18 s with mixed voices (5.62 → 8.33 %) and adds a new pair at or above 20 % on dev-other.
- `adaptReference` accepts 2 more bystander-only utterances as the caller.
- C7 saves caller words but costs bystander words. Its takeover detection misses 17.5-25.3 % of real takeovers, and the bystander words left inside caller finals roughly double (dev-clean bed 90 → 168, clean 153 → 294; dev-other bed 55 → 117, clean 88 → 179). That is about as many words, or more, than it saves (+0 to +112 per set). It does not help on Piper (12 → 12), because those words are lost to other→caller splits and drops, which C7 does not change. C7 also changes nothing with `splitOnSpeakerChange` off, the default.

**Decision:** no change ships. If this comes back, the target is whole-utterance drops on the default split-off path, and the few voices that cause most of them.
