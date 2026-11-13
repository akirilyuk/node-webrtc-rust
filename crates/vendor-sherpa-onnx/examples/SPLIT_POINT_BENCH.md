# Split-point bench: other → caller (chunk 1, plan 2026-11-13-speaker-split-other-to-caller)

**Status (2026-11-13): blocked on gate false accepts (plan decision g).** The paused set meets 0
caller-word cuts with stage 1. No stage-2 variant reaches 0 bystander words after the split on the
no-pause set. The cause is a gate false accept, not the split search (see "Why the no-pause set
leaks"). The split work waits for the false-accept bench (`FALSE_ACCEPT_BENCH.md`, chunk 1b) and
the gate change it recommends; the constants below are re-measured after that change.

## Decision h: prefer a few leaked words over clipping the caller

User decision 2026-11-13: with no pause, the stage-2 variant is the one with **0 clipped caller
words** and the fewest leaked bystander words. Both counts are reported in every table below.
From the combined table of the 300 + 300 run (stage 1 when its quiet run >= minimum, else stage 2),
the rows with 0 cuts on both sets are:

| quiet min ms | variant | paused cut / leak (scenes) | no pause cut / leak (scenes) |
|---|---|---|---|
| 500 | L1000/H125 start ±150 | 0 / 73 (6) | 0 / 167 (80) |
| 500 | L1000/H125 start ±100 | 0 / 73 (6) | 0 / 175 (88) |
| 500 | L1250/H250 start ±150 | 0 / 71 (6) | 0 / 204 (93) |
| 800 | L1000/H125 start ±150 | 0 / 73 (6) | 0 / 173 (89) |
| 1000 | L1000/H125 start ±150 | 0 / 73 (6) | 0 / 174 (90) |

Provisional pick under decision h: quiet-run minimum 500 ms, stage 2 on 1000 ms sub-windows with a
125 ms hop, split at the start of the first `caller` sub-window, refined by energy ±150 ms (stage 2
p99 184 ms per scene). It leaks because of the gate false accepts above, so it is not fixed until
the gate change from chunk 1b is in and this bench is re-run.

## Merge rate by bed (50 + 50 scenes, demo VAD: adaptive, 0.05, 200/1300 ms)

| bed | paused scenes merged into one utterance | no-pause merged |
|---|---|---|
| Tocumen -12 dB | 9 / 50 | 26 / 50 |
| Tocumen -10 dB | 11 / 50 | 23 / 50 |

With the default energy VAD, no paused scene merged at either level. The 300 + 300 run below is
at -12 dB, the demo's level.

## 300 + 300 scenes, Tocumen bed -12 dB

Bed: Tocumen airport CC0 (/tmp/sg-demo/bed/tocumen48.f32) at {-12} dB re the main voice. Scenes: /tmp/sg-split-n300-bed-12.

Threshold 0.459. Distance = split - caller voiced onset (ms) min / p50 / p95 / max. cut = caller words before the split (scenes), leak = bystander words at or after the split (scenes).

### Scene outcomes

| set | other→caller change | separate utterances (VAD split them) | window 1 already caller | merged, no caller window before the close | no VAD utterance on the caller |
|---|---|---|---|---|---|
| paused | 55 | 244 | 1 | 0 | 0 |
| no pause | 132 | 88 | 52 | 0 | 28 |

### Stage 1 alone (longest quiet run, any length)

| set | quiet run ms min / p50 / p95 / max | n | distance | cut | leak |
|---|---|---|---|---|---|
| paused | 160 / 480 / 940 / 1320 | 55 | -7308 / -669 / -13 / -9 | 0 (0) | 74 (6) |
| no pause | 120 / 320 / 620 / 820 | 132 | -4466 / -26 / 17 / 875 | 4 (2) | 214 (53) |


### Stage 2 alone (all 48 variants)
| variant | set | n | distance | cut | leak | no caller sub-window |
|---|---|---|---|---|---|---|
| L750/H250 start ±100 | paused | 55 | -7911 / -273 / 256 / 474 | 5 (5) | 76 (6) | 8 |
| L750/H250 start ±100 | no pause | 132 | -4923 / -89 / 439 / 559 | 22 (22) | 124 (31) | 9 |
| L750/H250 start ±150 | paused | 55 | -7981 / -269 / 266 / 502 | 4 (4) | 76 (6) | 8 |
| L750/H250 start ±150 | no pause | 132 | -4913 / -93 / 456 / 569 | 21 (21) | 118 (25) | 9 |
| L750/H250 mid+0 ±100 | paused | 55 | -7396 / 4 / 607 / 807 | 23 (17) | 71 (6) | 8 |
| L750/H250 mid+0 ±100 | no pause | 132 | -4466 / 204 / 808 / 837 | 104 (77) | 101 (11) | 9 |
| L750/H250 mid+0 ±150 | paused | 55 | -7386 / -18 / 617 / 797 | 20 (15) | 71 (6) | 8 |
| L750/H250 mid+0 ±150 | no pause | 132 | -4466 / 181 / 827 / 858 | 99 (70) | 100 (11) | 9 |
| L750/H250 mid+100 ±100 | paused | 55 | -7396 / 154 / 753 / 831 | 31 (23) | 71 (6) | 8 |
| L750/H250 mid+100 ±100 | no pause | 132 | -4466 / 371 / 826 / 1074 | 136 (98) | 100 (11) | 9 |
| L750/H250 mid+100 ±150 | paused | 55 | -7386 / 164 / 759 / 821 | 36 (26) | 71 (6) | 8 |
| L750/H250 mid+100 ±150 | no pause | 132 | -4466 / 401 / 839 / 1084 | 139 (91) | 100 (11) | 9 |
| L750/H250 mid+200 ±100 | paused | 55 | -7356 / 265 / 827 / 1089 | 51 (37) | 71 (6) | 8 |
| L750/H250 mid+200 ±100 | no pause | 132 | -4466 / 438 / 849 / 1095 | 175 (115) | 100 (11) | 9 |
| L750/H250 mid+200 ±150 | paused | 55 | -7386 / 321 / 803 / 1079 | 45 (31) | 70 (6) | 8 |
| L750/H250 mid+200 ±150 | no pause | 132 | -4466 / 450 / 996 / 1089 | 179 (112) | 100 (11) | 9 |
| L750/H250 mid+300 ±100 | paused | 55 | -7308 / 439 / 882 / 1093 | 63 (42) | 69 (6) | 8 |
| L750/H250 mid+300 ±100 | no pause | 132 | -4466 / 564 / 1089 / 1128 | 219 (121) | 99 (11) | 9 |
| L750/H250 mid+300 ±150 | paused | 55 | -7308 / 446 / 1079 / 1097 | 65 (38) | 69 (6) | 8 |
| L750/H250 mid+300 ±150 | no pause | 132 | -4466 / 557 / 1085 / 1178 | 217 (120) | 100 (11) | 9 |
| L750/H250 last-other-end ±100 | paused | 55 | -7391 / 199 / 756 / 854 | 39 (29) | 71 (6) | 8 |
| L750/H250 last-other-end ±100 | no pause | 132 | -4923 / 396 / 841 / 1079 | 149 (104) | 102 (11) | 9 |
| L750/H250 last-other-end ±150 | paused | 55 | -7401 / 151 / 802 / 1084 | 38 (27) | 71 (6) | 8 |
| L750/H250 last-other-end ±150 | no pause | 132 | -4913 / 417 / 845 / 1089 | 152 (98) | 102 (11) | 9 |
| L1000/H250 start ±100 | paused | 55 | -7911 / -580 / -43 / 2 | 0 (0) | 73 (6) | 7 |
| L1000/H250 start ±100 | no pause | 132 | -4923 / -318 / -16 / 462 | 2 (2) | 157 (76) | 4 |
| L1000/H250 start ±150 | paused | 55 | -7981 / -588 / -67 / -22 | 0 (0) | 73 (6) | 7 |
| L1000/H250 start ±150 | no pause | 132 | -4913 / -296 / -26 / 532 | 2 (2) | 144 (66) | 4 |
| L1000/H250 mid+0 ±100 | paused | 55 | -7391 / -91 / 466 / 568 | 8 (8) | 66 (6) | 7 |
| L1000/H250 mid+0 ±100 | no pause | 132 | -4343 / 154 / 515 / 842 | 63 (61) | 75 (8) | 4 |
| L1000/H250 mid+0 ±150 | paused | 55 | -7401 / -144 / 456 / 612 | 9 (8) | 65 (6) | 7 |
| L1000/H250 mid+0 ±150 | no pause | 132 | -4353 / 125 / 524 / 1072 | 54 (50) | 74 (8) | 4 |
| L1000/H250 mid+100 ±100 | paused | 55 | -7308 / -21 / 566 / 802 | 15 (11) | 64 (6) | 7 |
| L1000/H250 mid+100 ±100 | no pause | 132 | -4343 / 249 / 769 / 1082 | 105 (87) | 72 (8) | 4 |
| L1000/H250 mid+100 ±150 | paused | 55 | -7381 / -49 / 552 / 612 | 12 (9) | 65 (6) | 7 |
| L1000/H250 mid+100 ±150 | no pause | 132 | -4353 / 256 / 806 / 1092 | 100 (80) | 73 (8) | 4 |
| L1000/H250 mid+200 ±100 | paused | 55 | -7308 / 126 / 637 / 806 | 28 (21) | 63 (6) | 7 |
| L1000/H250 mid+200 ±100 | no pause | 132 | -4043 / 397 / 814 / 1082 | 138 (107) | 70 (8) | 4 |
| L1000/H250 mid+200 ±150 | paused | 55 | -7308 / -8 / 756 / 798 | 25 (18) | 63 (6) | 7 |
| L1000/H250 mid+200 ±150 | no pause | 132 | -3993 / 432 / 822 / 1092 | 141 (102) | 69 (8) | 4 |
| L1000/H250 mid+300 ±100 | paused | 55 | -7308 / 243 / 768 / 842 | 42 (33) | 63 (6) | 7 |
| L1000/H250 mid+300 ±100 | no pause | 132 | -3963 / 453 / 820 / 1202 | 173 (119) | 70 (8) | 4 |
| L1000/H250 mid+300 ±150 | paused | 55 | -7308 / 161 / 764 / 813 | 38 (27) | 63 (6) | 7 |
| L1000/H250 mid+300 ±150 | no pause | 132 | -3973 / 476 / 829 / 1092 | 178 (117) | 69 (8) | 4 |
| L1000/H250 last-other-end ±100 | paused | 55 | -7308 / 161 / 756 / 798 | 35 (27) | 63 (6) | 7 |
| L1000/H250 last-other-end ±100 | no pause | 132 | -4923 / 434 / 826 / 1092 | 159 (116) | 72 (8) | 4 |
| L1000/H250 last-other-end ±150 | paused | 55 | -7308 / 147 / 768 / 842 | 33 (24) | 63 (6) | 7 |
| L1000/H250 last-other-end ±150 | no pause | 132 | -4913 / 447 / 820 / 1082 | 163 (112) | 73 (8) | 4 |
| L1000/H125 start ±100 | paused | 55 | -7976 / -648 / -223 / -32 | 0 (0) | 73 (6) | 6 |
| L1000/H125 start ±100 | no pause | 132 | -4923 / -416 / -61 / -8 | 0 (0) | 182 (98) | 2 |
| L1000/H125 start ±150 | paused | 55 | -7966 / -638 / -172 / -22 | 0 (0) | 73 (6) | 6 |
| L1000/H125 start ±150 | no pause | 132 | -4913 / -441 / -78 / -26 | 0 (0) | 174 (90) | 2 |
| L1000/H125 mid+0 ±100 | paused | 55 | -7396 / -166 / 417 / 568 | 6 (6) | 65 (6) | 6 |
| L1000/H125 mid+0 ±100 | no pause | 132 | -4343 / -8 / 490 / 569 | 39 (39) | 70 (8) | 2 |
| L1000/H125 mid+0 ±150 | paused | 55 | -7386 / -173 / 277 / 558 | 5 (5) | 64 (6) | 6 |
| L1000/H125 mid+0 ±150 | no pause | 132 | -4353 / -23 / 486 / 606 | 28 (27) | 69 (8) | 2 |
| L1000/H125 mid+100 ±100 | paused | 55 | -7396 / -74 / 438 / 568 | 9 (9) | 63 (6) | 6 |
| L1000/H125 mid+100 ±100 | no pause | 132 | -4343 / 176 / 540 / 792 | 70 (68) | 66 (8) | 2 |
| L1000/H125 mid+100 ±150 | paused | 55 | -7386 / -86 / 456 / 587 | 10 (9) | 64 (6) | 6 |
| L1000/H125 mid+100 ±150 | no pause | 132 | -4353 / 159 / 537 / 822 | 61 (56) | 67 (8) | 2 |
| L1000/H125 mid+200 ±100 | paused | 55 | -7356 / -18 / 534 / 806 | 15 (12) | 63 (6) | 6 |
| L1000/H125 mid+200 ±100 | no pause | 132 | -4043 / 320 / 624 / 847 | 113 (96) | 62 (8) | 2 |
| L1000/H125 mid+200 ±150 | paused | 55 | -7386 / -46 / 607 / 798 | 16 (10) | 62 (6) | 6 |
| L1000/H125 mid+200 ±150 | no pause | 132 | -3993 / 341 / 622 / 837 | 114 (91) | 61 (8) | 2 |
| L1000/H125 mid+300 ±100 | paused | 55 | -7308 / 150 / 632 / 826 | 33 (26) | 61 (6) | 6 |
| L1000/H125 mid+300 ±100 | no pause | 132 | -3963 / 422 / 812 / 859 | 148 (115) | 62 (8) | 2 |
| L1000/H125 mid+300 ±150 | paused | 55 | -7308 / 129 / 607 / 798 | 23 (17) | 61 (6) | 6 |
| L1000/H125 mid+300 ±150 | no pause | 132 | -3973 / 439 / 808 / 840 | 152 (113) | 61 (8) | 2 |
| L1000/H125 last-other-end ±100 | paused | 55 | -7308 / 238 / 627 / 843 | 41 (34) | 61 (6) | 6 |
| L1000/H125 last-other-end ±100 | no pause | 132 | -4923 / 450 / 814 / 1067 | 165 (118) | 65 (8) | 2 |
| L1000/H125 last-other-end ±150 | paused | 55 | -7308 / 228 / 759 / 1093 | 41 (31) | 61 (6) | 6 |
| L1000/H125 last-other-end ±150 | no pause | 132 | -4913 / 492 / 817 / 1075 | 180 (120) | 65 (8) | 2 |
| L1250/H250 start ±100 | paused | 55 | -7911 / -834 / -458 / -142 | 0 (0) | 72 (6) | 5 |
| L1250/H250 start ±100 | no pause | 132 | -4707 / -522 / -181 / -74 | 0 (0) | 225 (111) | 1 |
| L1250/H250 start ±150 | paused | 55 | -7981 / -837 / -417 / -114 | 0 (0) | 71 (6) | 5 |
| L1250/H250 start ±150 | no pause | 132 | -4717 / -530 / -161 / -16 | 0 (0) | 220 (104) | 1 |
| L1250/H250 mid+0 ±100 | paused | 55 | -7146 / -239 / 147 / 523 | 3 (3) | 56 (6) | 5 |
| L1250/H250 mid+0 ±100 | no pause | 132 | -4142 / -10 / 504 / 569 | 43 (43) | 65 (10) | 1 |
| L1250/H250 mid+0 ±150 | paused | 55 | -7356 / -228 / 128 / 553 | 2 (2) | 59 (6) | 5 |
| L1250/H250 mid+0 ±150 | no pause | 132 | -4132 / -25 / 504 / 569 | 33 (31) | 66 (11) | 1 |
| L1250/H250 mid+100 ±100 | paused | 55 | -7146 / -139 / 259 / 563 | 5 (5) | 55 (6) | 5 |
| L1250/H250 mid+100 ±100 | no pause | 132 | -3942 / 187 / 512 / 569 | 73 (72) | 63 (9) | 1 |
| L1250/H250 mid+100 ±150 | paused | 55 | -7136 / -139 / 269 / 553 | 5 (5) | 54 (6) | 5 |
| L1250/H250 mid+100 ±150 | no pause | 132 | -3932 / 149 / 541 / 569 | 61 (54) | 63 (9) | 1 |
| L1250/H250 mid+200 ±100 | paused | 55 | -7078 / -50 / 397 / 563 | 9 (9) | 52 (6) | 5 |
| L1250/H250 mid+200 ±100 | no pause | 132 | -3942 / 306 / 587 / 789 | 106 (92) | 60 (9) | 1 |
| L1250/H250 mid+200 ±150 | paused | 55 | -7136 / -56 / 427 / 553 | 7 (6) | 54 (6) | 5 |
| L1250/H250 mid+200 ±150 | no pause | 132 | -3932 / 326 / 598 / 817 | 111 (90) | 60 (8) | 1 |
| L1250/H250 mid+300 ±100 | paused | 55 | -7078 / 11 / 447 / 803 | 23 (19) | 52 (6) | 5 |
| L1250/H250 mid+300 ±100 | no pause | 132 | -3782 / 423 / 809 / 858 | 149 (107) | 56 (7) | 1 |
| L1250/H250 mid+300 ±150 | paused | 55 | -7078 / -13 / 449 / 753 | 14 (12) | 52 (6) | 5 |
| L1250/H250 mid+300 ±150 | no pause | 132 | -3792 / 442 / 815 / 868 | 153 (108) | 56 (7) | 1 |
| L1250/H250 last-other-end ±100 | paused | 55 | -7078 / 159 / 474 / 798 | 29 (25) | 51 (6) | 5 |
| L1250/H250 last-other-end ±100 | no pause | 132 | -3647 / 467 / 822 / 893 | 175 (118) | 56 (8) | 1 |
| L1250/H250 last-other-end ±150 | paused | 55 | -7078 / 126 / 603 / 826 | 27 (22) | 52 (6) | 5 |
| L1250/H250 last-other-end ±150 | no pause | 132 | -3797 / 497 / 820 / 883 | 183 (117) | 58 (8) | 1 |

### Stage 2 embed time

| sub-window/hop | embeds per scene p50 / max | per embed ms p50 / p99 / max | per scene total ms p50 / p99 / max |
|---|---|---|---|
| 750/250 | 10 / 10 | 8.4 / 9.1 / 9.6 | 84 / 87 / 89 |
| 1000/250 | 9 / 9 | 10.6 / 11.5 / 11.9 | 96 / 98 / 99 |
| 1000/125 | 17 / 17 | 10.6 / 11.4 / 13.6 | 181 / 184 / 185 |
| 1250/250 | 8 / 8 | 12.5 / 13.3 / 13.8 | 100 / 103 / 103 |


## Why the no-pause set leaks

The leaking scenes are almost all bystander `en_US-libritts_r-medium` (15 of the 17 scenes where
the first `caller` window starts more than 800 ms before the bystander's end; the other two are
hfc_female and joe). On a 1.5 s window that voice scores 0.47–0.56, just above the 0.459
threshold, while it is still speaking. With the 1-window confirm (decision d), the change is
confirmed inside the bystander's speech. The search range `[start(k-1), start(k)+300]` then lies
before the bystander's end, so every split point inside it leaves bystander words in U2. That is
a false accept at the gate threshold, and no split-point variant can fix it. Examples: `s4_test_n46`
window scores 0.45, 0.53, 0.51, 0.76 (bystander ends at 12.37 s, caller window at 9.08 s);
`s4_test_p147` 0.36, 0.46, 0.40, 0.34, -0.04, 0.56, 0.73.

The same scenes give the 6 paused scenes with leaks in the stage 1 row.
