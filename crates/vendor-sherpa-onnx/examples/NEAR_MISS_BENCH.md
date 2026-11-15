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

| set | utterances | reference words | STT floor (words measure A misses with no gate) | whole-utterance drops (window 1 other, no confirm): utterances / words above floor | other-to-caller splits today |
| --- | --- | --- | --- | --- | --- |
| Piper straddle | 355 | 1298 | 0 | 0 / 0 | 355 |
| LibriSpeech straddle, Tocumen bed -12 dB | 80 | 345 | 10 | 11 / 41 | 69 |
| LibriSpeech straddle, clean | 118 | 496 | 0 | 8 / 26 | 110 |
| Piper fixtures, merged no-pause | 250 | 2881 | 0 | 0 / 0 | 250 |
| Piper caller-only, fixture babble | 600 | 19211 | 1243 | 0 / 0 | 0 |
| Piper caller-only, fixture babble + Tocumen bed -12 dB | 601 | 19205 | 1233 | 0 / 0 | 4 |
| LibriSpeech dev-clean caller-only, Tocumen bed -12 dB | 261 | 4698 | 26 | 21 / 220 | 26 |
| LibriSpeech dev-clean caller-only, clean | 252 | 4747 | 0 | 16 / 190 | 9 |

### Per set (split lost above floor / leaked words / splits kept / splits upgraded)

| rule | Piper straddle | LibriSpeech straddle, Tocumen bed -12 dB | LibriSpeech straddle, clean | Piper fixtures, merged no-pause | Piper caller-only, fixture babble | Piper caller-only, fixture babble + Tocumen bed -12 dB | LibriSpeech dev-clean caller-only, Tocumen bed -12 dB | LibriSpeech dev-clean caller-only, clean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| today: always split | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 44 / 250 / 0 | 0 / 0 / 0 / 0 | 3 / 0 / 4 / 0 | 98 / 0 / 26 / 0 | 51 / 0 / 9 / 0 |
| never split (always upgrade) | 0 / 4325 / 0 / 355 | 0 / 789 / 0 / 69 | 0 / 1284 / 0 / 110 | 0 / 4257 / 0 / 250 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 0 / 0 / 0 / 26 | 0 / 0 / 0 / 9 |
| R2: U1 has >= 2 other windows | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 55 / 248 / 2 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 62 / 0 / 5 / 21 | 35 / 0 / 3 / 6 |
| R1': a U1 window < 0.20 | 0 / 396 / 329 / 26 | 4 / 110 / 62 / 7 | 6 / 131 / 101 / 9 | 18 / 477 / 223 / 27 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 0 / 0 / 0 / 26 | 17 / 0 / 1 / 8 |
| R3': a U1 window < 0.20, or >= 2 other windows | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 55 / 248 / 2 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 62 / 0 / 5 / 21 | 35 / 0 / 3 / 6 |
| R1': a U1 window < 0.25 | 0 / 166 / 349 / 6 | 4 / 72 / 65 / 4 | 6 / 85 / 104 / 6 | 19 / 224 / 238 / 12 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 0 / 0 / 0 / 26 | 19 / 0 / 2 / 7 |
| R3': a U1 window < 0.25, or >= 2 other windows | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 55 / 248 / 2 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 62 / 0 / 5 / 21 | 35 / 0 / 3 / 6 |
| R1: a U1 window < 0.30 | 0 / 128 / 352 / 3 | 4 / 26 / 68 / 1 | 6 / 42 / 108 / 2 | 21 / 44 / 250 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 46 / 0 / 6 / 20 | 19 / 0 / 2 / 7 |
| R3 (= R4): a U1 window < 0.30, or >= 2 other windows | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 44 / 250 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 68 / 0 / 8 / 18 | 35 / 0 / 3 / 6 |
| R1': a U1 window < 0.35 | 0 / 93 / 355 / 0 | 4 / 26 / 68 / 1 | 6 / 17 / 110 / 0 | 21 / 44 / 250 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 52 / 0 / 8 / 18 | 35 / 0 / 3 / 6 |
| R3': a U1 window < 0.35, or >= 2 other windows | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 44 / 250 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 4 | 68 / 0 / 9 / 17 | 35 / 0 / 3 / 6 |
| R1': a U1 window < 0.40 | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 44 / 250 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 1 / 3 | 82 / 0 / 16 / 10 | 46 / 0 / 6 / 3 |
| R3': a U1 window < 0.40, or >= 2 other windows | 0 / 93 / 355 / 0 | 4 / 15 / 69 / 0 | 6 / 17 / 110 / 0 | 21 / 44 / 250 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 1 / 3 | 82 / 0 / 16 / 10 | 46 / 0 / 6 / 3 |

### Ranked (split-dependent lost words above the floor, both sets; then leaked words)

| rank | rule | caller-only lost above floor | caller-only false splits left | bystander->caller lost above floor | true splits upgraded | leaked words | total lost above floor |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | never split (always upgrade) | 0 | 0 | 0 | 784 | 10655 | 0 |
| 2 | R1': a U1 window < 0.20 | 17 | 1 | 28 | 69 | 1114 | 45 |
| 3 | R1': a U1 window < 0.25 | 19 | 2 | 29 | 28 | 547 | 48 |
| 4 | R1: a U1 window < 0.30 | 65 | 8 | 31 | 6 | 240 | 96 |
| 5 | R1': a U1 window < 0.35 | 87 | 11 | 31 | 1 | 180 | 118 |
| 6 | R2: U1 has >= 2 other windows | 97 | 8 | 31 | 2 | 180 | 128 |
| 7 | R3': a U1 window < 0.20, or >= 2 other windows | 97 | 8 | 31 | 2 | 180 | 128 |
| 8 | R3': a U1 window < 0.25, or >= 2 other windows | 97 | 8 | 31 | 2 | 180 | 128 |
| 9 | R3 (= R4): a U1 window < 0.30, or >= 2 other windows | 103 | 11 | 31 | 0 | 169 | 134 |
| 10 | R3': a U1 window < 0.35, or >= 2 other windows | 103 | 12 | 31 | 0 | 169 | 134 |
| 11 | R1': a U1 window < 0.40 | 128 | 23 | 31 | 0 | 169 | 159 |
| 12 | R3': a U1 window < 0.40, or >= 2 other windows | 128 | 23 | 31 | 0 | 169 | 159 |
| 13 | today: always split | 152 | 39 | 31 | 0 | 169 | 183 |

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

