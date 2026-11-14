# Caller → other noise-window bench (chunk 6c, plan 2026-11-13-speaker-split-other-to-caller, decisions n and o)

**Status (2026-11-14): no candidate meets decision o's bar.** The bar is 0 lost real takeovers, 0 bed splits and every bed tail blocked. None of the three signals separates real bystander-takeover windows from bed-only windows after a short caller phrase: on every signal the tail `other` windows reach values above the lowest takeover window (negative margin in the signal table). Closest two:

| candidate                        | lost takeovers (of 175) | delayed (+1500 ms p50, max 3000) | tails let through (of 187) |
| -------------------------------- | ----------------------- | -------------------------------- | -------------------------- |
| today's gate (no rule)           | 0                       | 0                                | 45                         |
| energy share D = 12 dB, min 0.30 | 1                       | 14                               | 8                          |
| energy share D = 12 dB, min 0.35 | 9                       | 22                               | 5                          |

The lowest threshold that blocks every tail (D = 12 @ 0.45) loses 20 of 175 takeovers. The VAD-active share is worse than the energy share at every threshold (at 0.30 it loses 25 and lets 18 tails through). The score signal (exploratory) does not separate the two either.

Per-window cost: the energy share takes about 16 µs at p99 per window, 0.003 % of the 500 ms verify budget. A TitaNet embed takes 18.3 ms at p99. The VAD-active share needs no extra computation. It would need an `agent.rs` change, though: today `gate.push_frame(&mono_bytes)` does not carry the per-frame `active` flag that `process_webrtc_pcm` returns.

## Setup

```text
cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
  --models <.models/speaker> --main-tts <.models/vits-piper-en_US-amy-low> \
  --phrases <stt-phrase-qualification.json> --out /tmp/sg-c2o --caller-to-other 125 \
  --s4-bed /tmp/sg-demo/bed/tocumen48.f32 --s4-bed-db -12
cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example caller_to_other_bench -- \
  --fixtures /tmp/sg-c2o --model <.models/speaker/nemo_en_titanet_small.onnx>
```

The run has 375 takeover scenes (125 at each bystander level, 25 per level and voice) and 187 tail scenes. Each tail is an S3 phrase followed by 8 s of bed only. All scenes play over the Tocumen bed at -12 dB. The bench uses the demo's adaptive VAD, the 1500 ms window grid with 300 ms pre-roll, threshold 0.459 and the 2-window confirm, and it counts at most 12 windows. Windows below a rule's threshold are booked as no evidence: they neither confirm nor reset a change, and they do not count toward the 12. The main table leaves out amy-low ← libritts_r 120 (51.5 % bed FAR in `FALSE_ACCEPT_BENCH.md`). That pair has its own row in the voice-pair tables.

Of the 375 takeovers, 93 are `separate`: the VAD closed before the bystander began, so the bystander starts a new key. Of the remaining 282, today's gate confirms the change on 175 main-pair takeovers and misses 47. A miss means the utterance ends before a second `other` window. The rest are the excluded pair.

## Bench output

### Scene outcomes

| set      | scored (window 1 caller) | no VAD utterance on the caller | separate (VAD closed before the bystander) | window 1 other | no full window |
| -------- | ------------------------ | ------------------------------ | ------------------------------------------ | -------------- | -------------- |
| takeover | 282                      | 0                              | 93                                         | 0              | 0              |
| tail     | 187                      | 0                              | 0                                          | 0              | 0              |

### Signal per window: takeover confirming windows vs tail `other` windows

Takeover: the two windows that confirm today's change (main pairs, both windows overlap or follow the bystander's onset). Tail: every `other` window after the caller label.

| signal              | takeover min / p05 / p50 | tail p50 / p95 / max  | margin (takeover min - tail max) |
| ------------------- | ------------------------ | --------------------- | -------------------------------- |
| energy D=12         | 0.144 / 0.311 / 0.578    | 0.222 / 0.511 / 0.644 | -0.500                           |
| energy D=18         | 0.267 / 0.489 / 0.822    | 0.511 / 0.833 / 0.922 | -0.656                           |
| energy D=24         | 0.433 / 0.756 / 0.967    | 0.844 / 0.989 / 1.000 | -0.567                           |
| VAD-active share    | 0.022 / 0.189 / 0.589    | 0.256 / 0.667 / 0.811 | -0.789                           |
| score (exploratory) | -0.059 / 0.007 / 0.121   | 0.050 / 0.429 / 0.459 | -0.601 (tail min - takeover max) |

### Candidates (main pairs; amy-low ← libritts_r 120 excluded)

| signal              | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |
| ------------------- | --------- | -------------------- | ---- | ---- | ------- | ------------------ | ----------------- | ------------------ | ----------------- | ---------------------- |
| energy D=12         | 0.000     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=12         | 0.050     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 145 / 187          | 42                | 47                     |
| energy D=12         | 0.100     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 152 / 187          | 35                | 47                     |
| energy D=12         | 0.150     | 175                  | 175  | 0    | 1       | 1500 / 1500        | 0                 | 159 / 187          | 28                | 47                     |
| energy D=12         | 0.200     | 175                  | 175  | 0    | 3       | 1500 / 1500        | 0                 | 164 / 187          | 23                | 47                     |
| energy D=12         | 0.250     | 175                  | 175  | 0    | 7       | 1500 / 1500        | 0                 | 173 / 187          | 14                | 47                     |
| energy D=12         | 0.300     | 175                  | 174  | 1    | 14      | 1500 / 3000        | 0                 | 179 / 187          | 8                 | 47                     |
| energy D=12         | 0.350     | 175                  | 166  | 9    | 22      | 1500 / 3000        | 0                 | 182 / 187          | 5                 | 47                     |
| energy D=12         | 0.400     | 175                  | 161  | 14   | 25      | 1500 / 3000        | 0                 | 184 / 187          | 3                 | 47                     |
| energy D=12         | 0.450     | 175                  | 155  | 20   | 41      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.500     | 175                  | 145  | 30   | 50      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.550     | 175                  | 118  | 57   | 52      | 1500 / 4500        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.600     | 175                  | 94   | 81   | 52      | 1500 / 4500        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.650     | 175                  | 71   | 104  | 44      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.700     | 175                  | 45   | 130  | 31      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.750     | 175                  | 22   | 153  | 16      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.800     | 175                  | 8    | 167  | 6       | 1500 / 1500        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.850     | 175                  | 1    | 174  | 1       | 1500 / 1500        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.900     | 175                  | 0    | 175  | 0       | 0 / 0              | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=12         | 0.950     | 175                  | 0    | 175  | 0       | 0 / 0              | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=18         | 0.000     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=18         | 0.050     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=18         | 0.100     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=18         | 0.150     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 143 / 187          | 44                | 47                     |
| energy D=18         | 0.200     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 145 / 187          | 42                | 47                     |
| energy D=18         | 0.250     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 146 / 187          | 41                | 47                     |
| energy D=18         | 0.300     | 175                  | 175  | 0    | 1       | 1500 / 1500        | 0                 | 148 / 187          | 39                | 47                     |
| energy D=18         | 0.350     | 175                  | 175  | 0    | 3       | 1500 / 1500        | 0                 | 150 / 187          | 37                | 47                     |
| energy D=18         | 0.400     | 175                  | 175  | 0    | 4       | 1500 / 1500        | 0                 | 153 / 187          | 34                | 47                     |
| energy D=18         | 0.450     | 175                  | 175  | 0    | 10      | 1500 / 1500        | 0                 | 160 / 187          | 27                | 47                     |
| energy D=18         | 0.500     | 175                  | 173  | 2    | 16      | 1500 / 3000        | 0                 | 165 / 187          | 22                | 47                     |
| energy D=18         | 0.550     | 175                  | 172  | 3    | 21      | 1500 / 3000        | 0                 | 172 / 187          | 15                | 47                     |
| energy D=18         | 0.600     | 175                  | 166  | 9    | 28      | 1500 / 3000        | 0                 | 178 / 187          | 9                 | 47                     |
| energy D=18         | 0.650     | 175                  | 161  | 14   | 39      | 1500 / 3000        | 0                 | 180 / 187          | 7                 | 47                     |
| energy D=18         | 0.700     | 175                  | 155  | 20   | 56      | 1500 / 3000        | 0                 | 184 / 187          | 3                 | 47                     |
| energy D=18         | 0.750     | 175                  | 144  | 31   | 61      | 1500 / 3000        | 0                 | 186 / 187          | 1                 | 47                     |
| energy D=18         | 0.800     | 175                  | 129  | 46   | 71      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=18         | 0.850     | 175                  | 92   | 83   | 62      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=18         | 0.900     | 175                  | 59   | 116  | 44      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=18         | 0.950     | 175                  | 17   | 158  | 15      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| energy D=24         | 0.000     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.050     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.100     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.150     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.200     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.250     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.300     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.350     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| energy D=24         | 0.400     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 144 / 187          | 43                | 47                     |
| energy D=24         | 0.450     | 175                  | 175  | 0    | 2       | 1500 / 1500        | 0                 | 144 / 187          | 43                | 47                     |
| energy D=24         | 0.500     | 175                  | 175  | 0    | 2       | 1500 / 1500        | 0                 | 144 / 187          | 43                | 47                     |
| energy D=24         | 0.550     | 175                  | 175  | 0    | 6       | 1500 / 1500        | 0                 | 147 / 187          | 40                | 47                     |
| energy D=24         | 0.600     | 175                  | 174  | 1    | 7       | 1500 / 1500        | 0                 | 147 / 187          | 40                | 47                     |
| energy D=24         | 0.650     | 175                  | 174  | 1    | 8       | 1500 / 1500        | 0                 | 147 / 187          | 40                | 47                     |
| energy D=24         | 0.700     | 175                  | 173  | 2    | 11      | 1500 / 1500        | 0                 | 155 / 187          | 32                | 47                     |
| energy D=24         | 0.750     | 175                  | 173  | 2    | 15      | 1500 / 1500        | 0                 | 156 / 187          | 31                | 47                     |
| energy D=24         | 0.800     | 175                  | 172  | 3    | 24      | 1500 / 3000        | 0                 | 161 / 187          | 26                | 47                     |
| energy D=24         | 0.850     | 175                  | 166  | 9    | 39      | 1500 / 4500        | 0                 | 168 / 187          | 19                | 47                     |
| energy D=24         | 0.900     | 175                  | 157  | 18   | 55      | 1500 / 4500        | 0                 | 174 / 187          | 13                | 47                     |
| energy D=24         | 0.950     | 175                  | 120  | 55   | 57      | 1500 / 3000        | 0                 | 184 / 187          | 3                 | 47                     |
| VAD-active share    | 0.000     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| VAD-active share    | 0.050     | 175                  | 174  | 1    | 0       | 0 / 0              | 0                 | 146 / 187          | 41                | 47                     |
| VAD-active share    | 0.100     | 175                  | 169  | 6    | 0       | 0 / 0              | 0                 | 149 / 187          | 38                | 47                     |
| VAD-active share    | 0.150     | 175                  | 166  | 9    | 0       | 0 / 0              | 0                 | 156 / 187          | 31                | 47                     |
| VAD-active share    | 0.200     | 175                  | 161  | 14   | 3       | 1500 / 1500        | 0                 | 158 / 187          | 29                | 47                     |
| VAD-active share    | 0.250     | 175                  | 157  | 18   | 7       | 1500 / 1500        | 0                 | 163 / 187          | 24                | 47                     |
| VAD-active share    | 0.300     | 175                  | 150  | 25   | 10      | 1500 / 1500        | 0                 | 169 / 187          | 18                | 47                     |
| VAD-active share    | 0.350     | 175                  | 148  | 27   | 15      | 1500 / 3000        | 0                 | 172 / 187          | 15                | 47                     |
| VAD-active share    | 0.400     | 175                  | 143  | 32   | 19      | 1500 / 3000        | 0                 | 175 / 187          | 12                | 47                     |
| VAD-active share    | 0.450     | 175                  | 136  | 39   | 28      | 1500 / 3000        | 0                 | 179 / 187          | 8                 | 47                     |
| VAD-active share    | 0.500     | 175                  | 120  | 55   | 37      | 1500 / 3000        | 0                 | 181 / 187          | 6                 | 47                     |
| VAD-active share    | 0.550     | 175                  | 103  | 72   | 35      | 1500 / 3000        | 0                 | 185 / 187          | 2                 | 47                     |
| VAD-active share    | 0.600     | 175                  | 91   | 84   | 43      | 1500 / 3000        | 0                 | 186 / 187          | 1                 | 47                     |
| VAD-active share    | 0.650     | 175                  | 68   | 107  | 36      | 1500 / 3000        | 0                 | 186 / 187          | 1                 | 47                     |
| VAD-active share    | 0.700     | 175                  | 41   | 134  | 26      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| VAD-active share    | 0.750     | 175                  | 24   | 151  | 14      | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| VAD-active share    | 0.800     | 175                  | 15   | 160  | 8       | 1500 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| VAD-active share    | 0.850     | 175                  | 6    | 169  | 4       | 3000 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| VAD-active share    | 0.900     | 175                  | 3    | 172  | 1       | 3000 / 3000        | 0                 | 187 / 187          | 0                 | 47                     |
| VAD-active share    | 0.950     | 175                  | 1    | 174  | 0       | 0 / 0              | 0                 | 187 / 187          | 0                 | 47                     |
| score (exploratory) | 0.459     | 175                  | 175  | 0    | 0       | 0 / 0              | 0                 | 142 / 187          | 45                | 47                     |
| score (exploratory) | 0.400     | 175                  | 175  | 0    | 5       | 1500 / 1500        | 0                 | 143 / 187          | 44                | 47                     |
| score (exploratory) | 0.350     | 175                  | 174  | 1    | 16      | 1500 / 3000        | 0                 | 143 / 187          | 44                | 47                     |
| score (exploratory) | 0.300     | 175                  | 170  | 5    | 27      | 1500 / 3000        | 0                 | 143 / 187          | 44                | 47                     |
| score (exploratory) | 0.250     | 175                  | 156  | 19   | 30      | 1500 / 3000        | 0                 | 143 / 187          | 44                | 47                     |
| score (exploratory) | 0.200     | 175                  | 135  | 40   | 32      | 1500 / 3000        | 0                 | 143 / 187          | 44                | 47                     |
| score (exploratory) | 0.150     | 175                  | 119  | 56   | 38      | 1500 / 3000        | 0                 | 145 / 187          | 42                | 47                     |
| score (exploratory) | 0.100     | 175                  | 90   | 85   | 46      | 1500 / 4500        | 0                 | 149 / 187          | 38                | 47                     |
| score (exploratory) | 0.050     | 175                  | 41   | 134  | 29      | 1500 / 4500        | 0                 | 156 / 187          | 31                | 47                     |
| score (exploratory) | 0.000     | 175                  | 7    | 168  | 6       | 3000 / 4500        | 0                 | 179 / 187          | 8                 | 47                     |
| score (exploratory) | -0.050    | 175                  | 0    | 175  | 0       | 0 / 0              | 0                 | 186 / 187          | 1                 | 47                     |
| score (exploratory) | -0.100    | 175                  | 0    | 175  | 0       | 0 / 0              | 0                 | 187 / 187          | 0                 | 47                     |

Threshold 0 (score: 0.459) is today's gate.

### Bar (0 lost, 0 early, all tails blocked): 0 passing (signal, threshold)

- energy D=12: none
- energy D=18: none
- energy D=24: none
- VAD-active share: none
- score (exploratory): none

#### today's gate: by bystander level

| signal | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |
| ------ | --------- | -------------------- | ---- | ---- | ------- | ------------------ | ----------------- | ------------------ | ----------------- | ---------------------- |
| -6 dB  | 0.000     | 55                   | 55   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 17                     |
| -3 dB  | 0.000     | 53                   | 53   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 16                     |
| 0 dB   | 0.000     | 67                   | 67   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 14                     |

#### today's gate: by voice pair

| signal                                          | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |
| ----------------------------------------------- | --------- | -------------------- | ---- | ---- | ------- | ------------------ | ----------------- | ------------------ | ----------------- | ---------------------- |
| amy-low ← hfc_female                            | 0.000     | 39                   | 39   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 15                     |
| amy-low ← joe                                   | 0.000     | 53                   | 53   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 6                      |
| amy-low ← lessac                                | 0.000     | 44                   | 44   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 11                     |
| amy-low ← libritts_r 120 (excluded, FAR >= 5 %) | 0.000     | 29                   | 29   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 31                     |
| amy-low ← libritts_r 300                        | 0.000     | 39                   | 39   | 0    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 15                     |

#### energy D=12 @ 0.30: by bystander level

| signal | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |
| ------ | --------- | -------------------- | ---- | ---- | ------- | ------------------ | ----------------- | ------------------ | ----------------- | ---------------------- |
| -6 dB  | 0.300     | 55                   | 54   | 1    | 6       | 1500 / 3000        | 0                 | 0 / 0              | 0                 | 17                     |
| -3 dB  | 0.300     | 53                   | 53   | 0    | 2       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 16                     |
| 0 dB   | 0.300     | 67                   | 67   | 0    | 6       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 14                     |

#### energy D=12 @ 0.30: by voice pair

| signal                                          | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |
| ----------------------------------------------- | --------- | -------------------- | ---- | ---- | ------- | ------------------ | ----------------- | ------------------ | ----------------- | ---------------------- |
| amy-low ← hfc_female                            | 0.300     | 39                   | 39   | 0    | 1       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 15                     |
| amy-low ← joe                                   | 0.300     | 53                   | 52   | 1    | 4       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 6                      |
| amy-low ← lessac                                | 0.300     | 44                   | 44   | 0    | 5       | 1500 / 3000        | 0                 | 0 / 0              | 0                 | 11                     |
| amy-low ← libritts_r 120 (excluded, FAR >= 5 %) | 0.300     | 29                   | 27   | 2    | 0       | 0 / 0              | 0                 | 0 / 0              | 0                 | 31                     |
| amy-low ← libritts_r 300                        | 0.300     | 39                   | 39   | 0    | 4       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 15                     |

#### energy D=12 @ 0.35: by bystander level

| signal | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |
| ------ | --------- | -------------------- | ---- | ---- | ------- | ------------------ | ----------------- | ------------------ | ----------------- | ---------------------- |
| -6 dB  | 0.350     | 55                   | 48   | 7    | 11      | 1500 / 3000        | 0                 | 0 / 0              | 0                 | 17                     |
| -3 dB  | 0.350     | 53                   | 51   | 2    | 4       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 16                     |
| 0 dB   | 0.350     | 67                   | 67   | 0    | 7       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 14                     |

#### energy D=12 @ 0.35: by voice pair

| signal                                          | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |
| ----------------------------------------------- | --------- | -------------------- | ---- | ---- | ------- | ------------------ | ----------------- | ------------------ | ----------------- | ---------------------- |
| amy-low ← hfc_female                            | 0.350     | 39                   | 39   | 0    | 2       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 15                     |
| amy-low ← joe                                   | 0.350     | 53                   | 48   | 5    | 6       | 1500 / 3000        | 0                 | 0 / 0              | 0                 | 6                      |
| amy-low ← lessac                                | 0.350     | 44                   | 42   | 2    | 5       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 11                     |
| amy-low ← libritts_r 120 (excluded, FAR >= 5 %) | 0.350     | 29                   | 27   | 2    | 1       | 1500 / 1500        | 0                 | 0 / 0              | 0                 | 31                     |
| amy-low ← libritts_r 300                        | 0.350     | 39                   | 37   | 2    | 9       | 1500 / 3000        | 0                 | 0 / 0              | 0                 | 15                     |

### Per-window cost

Windows scored: 2784. Embed ms p50 / p99 / max: 16.8 / 18.3 / 19.1. Energy share (frame dB + p90 + share) µs p50 / p99 / max: 11.8 / 15.7 / 22.8, 0.0031 % of the 500 ms verify budget at p99. The VAD-active share reuses the flag the agent's VAD already computes per frame (no extra work, but `agent.rs` must pass it with each frame).
