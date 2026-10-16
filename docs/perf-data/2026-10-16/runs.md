# Runs behind the 0.9.48 vs 0.10.0 vs 0.11.2 comparison (2026-10-16)

- Host: Apple M4 Pro Mac (Darwin 24.6.0), Node v26.4.0, AC power, whole measurement phase under `caffeinate -s` (A-C one driver, D a second driver).
- Date: 2026-10-16. Measurement window: 07:47:11Z to 08:17:09Z (14:47:11 to 15:17:09 +07). Smoke runs and builds before 14:47 are not measurements.
- `pmset -g log` for the window: no Sleep / DarkWake / Wake entries. Only "Display is turned off/on" notifications (14:54:14, 14:54:45, 14:55:16, 14:56:18, 14:58:47) and caffeinate assertion lines. No run is inconclusive for host suspend.
- Native addon: the PUBLISHED `@node-webrtc-rust/bindings-darwin-arm64@<V>` `.node` copied to `packages/bindings/node-webrtc-rust.darwin-arm64.node` in each detached worktree (`release/<V>`); TS built with `scripts/ci/build-ts-workspace.sh`.
- Models: `examples/voice-agent-local-sherpa/.models` shared by the three checkouts (kroko en STT, vits-piper amy-low TTS, whisper-tiny for LID are all in it); nothing downloaded.
- Host load: runs went back to back (no cooldown), so the 1-minute load average printed before each run includes the previous run. Baseline ambient load was about 3-5 (Chrome, WindowServer, IDE). Per-run values are in `host-load.txt` (range 3.08 to 9.41; highest values (7.7-9.4) were the first A runs, right after the parallel `npm ci`/build phase). Order rotation spreads this across versions.
- Probe for A: `scripts/perf/js-thread-probe.mjs` from `main` for all three versions (0.9.48 does not ship it). The same script was used for all versions; roundtrip-load.ts is byte-identical in the three releases (same `perf_probe` fields).

## Binding sha256

```
f95ad9a2adc2ee2030fd13b4c571e852fdc191d231ee98b9ce41a52a55bebbad  nwr-0.9.48/packages/bindings/node-webrtc-rust.darwin-arm64.node
656cf262c8d916ef57890e695f812e7872feada79ac152e488b5a6a4c749d214  nwr-0.10.0/packages/bindings/node-webrtc-rust.darwin-arm64.node
0dff77b7c89d20d1b7af5af74370272a712289b78e2dfe24566a8063e4dd67e3  nwr-0.11.2/packages/bindings/node-webrtc-rust.darwin-arm64.node
```

## Table A — JS main-thread cost, 10 voice sessions (`start:roundtrip-counting-echo-lid-multi`, probe on the echo server)

Values = last cumulative probe line of the echo-server process (the process with the highest cum process CPU; the tsx wrapper process is ignored).

| Version | Round | JS-thread ms/s | Process CPU ms/s | ELU   | Delay p99 ms | Dur s | Verdict                                                                                                                        |
| ------- | ----- | -------------- | ---------------- | ----- | ------------ | ----- | ------------------------------------------------------------------------------------------------------------------------------ |
| 0.9.48  | 1     | 56             | 718              | 0.077 | 11.5         | 19    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |
| 0.9.48  | 2     | 53             | 697              | 0.073 | 11.4         | 19    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |
| 0.9.48  | 3     | 58             | 224              | 0.086 | 11.8         | 108   | FAIL exit 1: 7 sessions with word errors (s10,s2,s3,s4,s5,s7,s8): counting word 10 expected "ten", got "nine" (doubled "nine") |
| 0.10.0  | 1     | 19             | 580              | 0.029 | 11.7         | 17    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |
| 0.10.0  | 2     | 18             | 587              | 0.028 | 11.7         | 19    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |
| 0.10.0  | 3     | 19             | 558              | 0.030 | 11.8         | 18    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |
| 0.11.2  | 1     | 19             | 590              | 0.031 | 11.6         | 18    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |
| 0.11.2  | 2     | 18             | 552              | 0.028 | 11.6         | 17    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |
| 0.11.2  | 3     | 20             | 586              | 0.030 | 11.6         | 17    | PASS (10/10 full echo, 0 sessions with word errors)                                                                            |

Note: 0.9.48 round 3 ran 108 s instead of ~18 s (failed run, long tail), so its cumulative CPU figures are diluted over 105 s (process CPU 224 ms/s is not comparable). Its probe line at t=15s (same window as the other runs): JS-thread 54, process CPU 634, ELU 0.075, delay p99 11.5.

Median per version (3 runs, last-line values / with 0.9.48 r3 taken at t=15s):

| Version | JS-thread ms/s | Process CPU ms/s | ELU           | Delay p99 ms |
| ------- | -------------- | ---------------- | ------------- | ------------ |
| 0.9.48  | 56 / 54        | 697 / 697        | 0.077 / 0.075 | 11.5 / 11.5  |
| 0.10.0  | 19 / 19        | 580 / 580        | 0.029 / 0.029 | 11.7 / 11.7  |
| 0.11.2  | 19 / 19        | 586 / 586        | 0.030 / 0.030 | 11.6 / 11.6  |

## Table B — voice loopback load, normal mode (8 legs x 5 turns, `start:roundtrip-load`)

| Version | Round | Final p50 ms | Final p95 ms | TTS start p50 ms | TTS start p95 ms | Turn success | CPU s / session-min | Event-loop lag p99 ms | RSS end MB | Wall s | Exit                                             |
| ------- | ----- | ------------ | ------------ | ---------------- | ---------------- | ------------ | ------------------- | --------------------- | ---------- | ------ | ------------------------------------------------ |
| 0.9.48  | 1     | 2077.59      | 2185.81      | 70.93            | 498.34           | 1            | 4.71                | 1.86                  | 831.59     | 24.1   | 0                                                |
| 0.9.48  | 2     | 2036.77      | 2201.02      | 71.98            | 449.74           | 0.975        | 4.46                | 1.62                  | 813.48     | 24.33  | 1 (turn success 0.975 < 1: 1 of 40 turns failed) |
| 0.9.48  | 3     | 2051.97      | 2173.85      | 63.77            | 471.76           | 1            | 4.45                | 1.84                  | 746.27     | 24.05  | 0                                                |
| 0.10.0  | 1     | 2054.92      | 2175.69      | 68.16            | 445.08           | 1            | 4.28                | 1.56                  | 787.94     | 24.09  | 0                                                |
| 0.10.0  | 2     | 2055.24      | 2175.15      | 62.44            | 498.16           | 1            | 4.15                | 1.33                  | 768.77     | 24.21  | 0                                                |
| 0.10.0  | 3     | 2053.36      | 2201.7       | 63.78            | 424.21           | 1            | 4.13                | 1.41                  | 873.63     | 24.21  | 0                                                |
| 0.11.2  | 1     | 2061.26      | 2183.78      | 61.69            | 442.27           | 1            | 4.06                | 1.56                  | 833.22     | 24.18  | 0                                                |
| 0.11.2  | 2     | 2045.5       | 2178.58      | 67.89            | 476.76           | 1            | 4.12                | 1.32                  | 800.53     | 24.05  | 0                                                |
| 0.11.2  | 3     | 2037.56      | 2192.14      | 60.62            | 485.97           | 1            | 4.03                | 1.42                  | 696.73     | 24.13  | 0                                                |

Median per version:

| Version | Final p50 | Final p95 | TTS start p50 | TTS start p95 | Turn success | CPU s/session-min | Lag p99 | RSS end |
| ------- | --------- | --------- | ------------- | ------------- | ------------ | ----------------- | ------- | ------- |
| 0.9.48  | 2051.97   | 2185.81   | 70.93         | 471.76        | 1.00         | 4.46              | 1.84    | 813.48  |
| 0.10.0  | 2054.92   | 2175.69   | 63.78         | 445.08        | 1.00         | 4.15              | 1.41    | 787.94  |
| 0.11.2  | 2045.50   | 2183.78   | 61.69         | 476.76        | 1.00         | 4.06              | 1.42    | 800.53  |

## Table C — capacity mode (`SHERPA_LOAD_CAPACITY=1 SHERPA_LOAD_TURNS=2`)

A step passes while turn success is 1 and final p95 <= baseline p95 (step 2 legs) + 1000 ms. Capacity runs exit 0 even when a step fails (by design).

### 0.9.48 round 1 — max_legs_at_slo=32 (exit 0)

| Legs | Passed | Final p50 | Final p95 | CPU s/session-min | RSS end MB | Turn success | TTS start p95 |
| ---- | ------ | --------- | --------- | ----------------- | ---------- | ------------ | ------------- |
| 2    | True   | 1909.14   | 2059.83   | 7.57              | 596.72     | 1            | 464.45        |
| 4    | True   | 2020.11   | 2135.72   | 4.41              | 655.48     | 1            | 75.1          |
| 6    | True   | 2070.69   | 2232.4    | 4.26              | 710.86     | 1            | 82.33         |
| 8    | True   | 2043.76   | 2178.84   | 3.96              | 779.56     | 1            | 75.12         |
| 10   | True   | 2053.1    | 2165.15   | 3.9               | 840.08     | 1            | 82.51         |
| 12   | True   | 2075.32   | 2176.03   | 3.72              | 871.45     | 1            | 81.65         |
| 16   | True   | 2083.85   | 2226.01   | 3.64              | 920.97     | 1            | 105           |
| 20   | True   | 2098.79   | 2193.58   | 3.54              | 998.84     | 1            | 133.83        |
| 24   | True   | 2112.91   | 2245.52   | 3.5               | 1059.95    | 1            | 270.5         |
| 32   | True   | 2121.29   | 2322.05   | 3.48              | 1153.72    | 1            | 453.73        |
| 40   | False  | 2436.72   | 3081.25   | 3.26              | 1260.97    | 1            | 652.73        |

Failing step: 40 legs: final p95 3081.25 > baseline+1000 = 3060

### 0.9.48 round 2 — max_legs_at_slo=16 (exit 0)

| Legs | Passed | Final p50 | Final p95 | CPU s/session-min | RSS end MB | Turn success | TTS start p95 |
| ---- | ------ | --------- | --------- | ----------------- | ---------- | ------------ | ------------- |
| 2    | True   | 1900.14   | 2058.08   | 7.79              | 686.34     | 1            | 519.48        |
| 4    | True   | 2043.75   | 2106.13   | 4.78              | 743.91     | 1            | 77.39         |
| 6    | True   | 2052.44   | 2192.82   | 4.12              | 814.41     | 1            | 80.14         |
| 8    | True   | 2050.53   | 2219.94   | 3.95              | 877.47     | 1            | 76.97         |
| 10   | True   | 2034.51   | 2164.16   | 3.87              | 917.13     | 1            | 73.98         |
| 12   | True   | 2087.24   | 2190.49   | 3.75              | 931.31     | 1            | 82.46         |
| 16   | True   | 2086.17   | 2205.1    | 3.65              | 972.06     | 1            | 97.16         |
| 20   | False  | 2103.31   | 2190.22   | 3.58              | 1035.64    | 0.975        | 131.16        |

Failing step: 20 legs: turn success 0.975 < 1

### 0.10.0 round 1 — max_legs_at_slo=16 (exit 0)

| Legs | Passed | Final p50 | Final p95 | CPU s/session-min | RSS end MB | Turn success | TTS start p95 |
| ---- | ------ | --------- | --------- | ----------------- | ---------- | ------------ | ------------- |
| 2    | True   | 1915.58   | 2031.39   | 7.3               | 620.27     | 1            | 529.88        |
| 4    | True   | 2019.83   | 2155.07   | 4.01              | 680.23     | 1            | 74.05         |
| 6    | True   | 2037.64   | 2170.82   | 3.5               | 727.05     | 1            | 73            |
| 8    | True   | 2033.41   | 2179.67   | 3.64              | 793.47     | 1            | 70.36         |
| 10   | True   | 2041.07   | 2166.79   | 3.6               | 831.83     | 1            | 71.99         |
| 12   | True   | 2048.03   | 2163.38   | 3.42              | 866.48     | 1            | 83.92         |
| 16   | True   | 2057.84   | 2195.21   | 3.43              | 918.27     | 1            | 94.13         |
| 20   | False  | 2113.32   | 2186.42   | 3.25              | 983.19     | 0.975        | 137.91        |

Failing step: 20 legs: turn success 0.975 < 1

### 0.10.0 round 2 — max_legs_at_slo=40 (exit 0)

| Legs | Passed | Final p50 | Final p95 | CPU s/session-min | RSS end MB | Turn success | TTS start p95 |
| ---- | ------ | --------- | --------- | ----------------- | ---------- | ------------ | ------------- |
| 2    | True   | 1908.28   | 2051      | 7.54              | 646.47     | 1            | 746.76        |
| 4    | True   | 2011.39   | 2121.18   | 4.06              | 671.66     | 1            | 71.5          |
| 6    | True   | 2048.93   | 2176.97   | 3.77              | 742.83     | 1            | 76.1          |
| 8    | True   | 2024.45   | 2166.47   | 3.72              | 799.97     | 1            | 72.07         |
| 10   | True   | 2067.17   | 2162.99   | 3.81              | 816.55     | 1            | 82.72         |
| 12   | True   | 2052.9    | 2172.34   | 3.66              | 881.47     | 1            | 87.21         |
| 16   | True   | 2065.33   | 2248.23   | 3.35              | 940.59     | 1            | 79.82         |
| 20   | True   | 2097.93   | 2244.85   | 3.28              | 983.08     | 1            | 137.39        |
| 24   | True   | 2112.57   | 2250.37   | 3.25              | 1051.3     | 1            | 245           |
| 32   | True   | 2182.12   | 2401.02   | 3.13              | 1166.95    | 1            | 441.14        |
| 40   | True   | 2433.47   | 2896.53   | 3.08              | 1273.53    | 1            | 648.89        |
| 48   | False  | 3309.87   | 4071.47   | 2.62              | 1328.05    | 1            | 844.46        |

Failing step: 48 legs: final p95 4071.47 > baseline+1000 = 3051

### 0.11.2 round 1 — max_legs_at_slo=40 (exit 0)

| Legs | Passed | Final p50 | Final p95 | CPU s/session-min | RSS end MB | Turn success | TTS start p95 |
| ---- | ------ | --------- | --------- | ----------------- | ---------- | ------------ | ------------- |
| 2    | True   | 1843.16   | 2063.51   | 6.76              | 714.53     | 1            | 476.78        |
| 4    | True   | 1994.32   | 2125.5    | 3.93              | 748.78     | 1            | 105.38        |
| 6    | True   | 2041.46   | 2168.05   | 3.84              | 783.42     | 1            | 82.86         |
| 8    | True   | 2013.99   | 2192.26   | 3.74              | 872.94     | 1            | 83.43         |
| 10   | True   | 2059.65   | 2155      | 3.41              | 898.33     | 1            | 87.44         |
| 12   | True   | 2100.79   | 2204.02   | 3.45              | 919.83     | 1            | 81.05         |
| 16   | True   | 2106.65   | 2173.86   | 3.32              | 970.8      | 1            | 92.27         |
| 20   | True   | 2122.72   | 2235.62   | 3.26              | 1015.52    | 1            | 133.15        |
| 24   | True   | 2144.39   | 2303.49   | 3.2               | 1060.08    | 1            | 263.61        |
| 32   | True   | 2163      | 2312.03   | 3.15              | 1168.73    | 1            | 437.21        |
| 40   | True   | 2218.34   | 2412.97   | 3.04              | 1257.52    | 1            | 653.94        |
| 48   | False  | 2852.64   | 3147.91   | 2.84              | 1354.5     | 0.9896       | 864.62        |

Failing step: 48 legs: turn success 0.9896 < 1; final p95 3147.91 > baseline+1000 = 3064

### 0.11.2 round 2 — max_legs_at_slo=40 (exit 0)

| Legs | Passed | Final p50 | Final p95 | CPU s/session-min | RSS end MB | Turn success | TTS start p95 |
| ---- | ------ | --------- | --------- | ----------------- | ---------- | ------------ | ------------- |
| 2    | True   | 1873.52   | 2042.05   | 7.22              | 632.83     | 1            | 465.8         |
| 4    | True   | 2005.75   | 2161.22   | 4.29              | 696.61     | 1            | 73.45         |
| 6    | True   | 2035.23   | 2172.67   | 3.83              | 734.23     | 1            | 72.92         |
| 8    | True   | 2026.27   | 2166.04   | 3.63              | 789.13     | 1            | 74.56         |
| 10   | True   | 2049.35   | 2168.18   | 3.5               | 812.44     | 1            | 71.45         |
| 12   | True   | 2041.77   | 2169.02   | 3.42              | 854.81     | 1            | 89.8          |
| 16   | True   | 2092.19   | 2280.78   | 3.33              | 892.45     | 1            | 98.64         |
| 20   | True   | 2098.28   | 2189.72   | 3.19              | 969.53     | 1            | 132.15        |
| 24   | True   | 2122.6    | 2222.23   | 3.22              | 1055.95    | 1            | 258.85        |
| 32   | True   | 2161.45   | 2314.71   | 3.11              | 1180.14    | 1            | 446.72        |
| 40   | True   | 2181.75   | 2402.72   | 3.09              | 1244.8     | 1            | 661.34        |
| 48   | False  | 2874.87   | 3189.12   | 2.84              | 1332.56    | 1            | 855.44        |

Failing step: 48 legs: final p95 3189.12 > baseline+1000 = 3042

Summary: max_legs_at_slo per run — 0.9.48: 32 / 16; 0.10.0: 16 / 40; 0.11.2: 40 / 40 (round 1 / round 2)

## Table D — CPU starvation, 5 ms run / 45 ms stop duty cycle, 10 sessions, 3 runs per version

Harness: copy of `run-starve.py` + `runtable.py` from the 2026-10-14 feature assets (run with `python3 -I`); counting as in runs.md: "Sessions w/ word errors" = sessions whose speaker transcript missed a number word or the prefix (the harness `inbound:` assertions); "LID order (gate bound)" is a separate failure class. Raw: `starve-5-45-<V>.txt`, `starve-5-45-<V>.table.md`; per-run logs in each worktree `.test-logs/starve/starve-5-45-<V>/`.

| Version | Run | Verdict | Duration s | Sessions w/ word errors | Other failure class |
| ------- | --- | ------- | ---------- | ----------------------- | ------------------- |
| 0.9.48  | 1   | FAIL    | 136        | 1 (s1)                  | none                |
| 0.9.48  | 2   | PASS    | 51         | 0                       | none                |
| 0.9.48  | 3   | PASS    | 51         | 0                       | none                |
| 0.10.0  | 1   | PASS    | 51         | 0                       | none                |
| 0.10.0  | 2   | PASS    | 53         | 0                       | none                |
| 0.10.0  | 3   | PASS    | 52         | 0                       | none                |
| 0.11.2  | 1   | PASS    | 51         | 0                       | none                |
| 0.11.2  | 2   | PASS    | 49         | 0                       | none                |
| 0.11.2  | 3   | PASS    | 50         | 0                       | none                |

0.9.48 run 1: s1 transcript "One, two, three, four five six seven eight nine ten" lost the "Okay" prefix (`missing any prefix before counting`). The run took 136 s (the passing runs took 51 s). No LID-order failure and no C1 wall closes in any of the 9 runs. The JS-thread/ELU/CPU probe columns are empty because the probe is not attached in this harness.

## Notes

- 0.9.48 failures (A round 3: doubled "nine" in 7 sessions; B round 2: 1 of 40 turns failed; C round 2: one misheard turn at the 20-leg step; D run 1: missing "Okay" prefix) were not re-run. 0.10.0 had one C step fail on turn success (20 legs, round 1). 0.11.2 failed only the designed capacity-limit step.
- The starvation duty cycle ran milder than on 2026-10-14 (runs took 49–53 s here, 77–109 s there) with the same harness and parameters. It separates the versions less sharply than the 2026-10-14 runs.

## Charts

`charts.json` holds the data of every chart in `../charts/`. Render them with:

```bash
node scripts/perf/perf-charts.mjs docs/perf-data/2026-10-16/charts.json docs/perf-data/charts
```
