# Benchmarks

Latency of the cloud speech vendors (TTS time to first audio, STT open, first partial and final),
measured against the vendors' real APIs by the vendor crates of this library. The CLI records
where it ran, so a run on your machine can be compared with ours.

## Latest results

Every cell is median / p90 in milliseconds, copied from the `summary.md` of the folder named above
the table. Cold is the first request or turn, warm is every later one
([how the two differ](METHODOLOGY.md#cold-and-warm)).

### Library, 2026-10-19: [`results/2026-10-19-ci-eu/`](results/2026-10-19-ci-eu/)

The vendor APIs called directly from a CI runner in a Hetzner data centre in the EU, 10 runs per
group, nwr 0.12.0. Source table: [`summary.md`](results/2026-10-19-ci-eu/summary.md).

TTS time to first audio (`ttfa_ms`), warm:

| Vendor     | Transport | `short`      | `sentence`    |
| ---------- | --------- | ------------ | ------------- |
| Cartesia   | http      | 129.5 / 142  | 108.5 / 119   |
| Cartesia   | websocket | 128 / 141    | 106 / 109     |
| ElevenLabs | http      | 585.5 / 597  | 727.5 / 763   |
| ElevenLabs | websocket | 363 / 380    | 504 / 514     |
| Fish Audio | http      | 290.5 / 304  | 291.5 / 317   |
| Fish Audio | websocket | 1021 / 1124  | 1023.5 / 1073 |
| Groq       | http      | 194 / 205    | 198 / 213     |
| OpenAI     | http      | 824.5 / 1263 | 1004 / 1250   |

The two text columns are the `short` and `sentence` texts of the run ([inputs](METHODOLOGY.md#inputs)). To pick a transport per
vendor from these numbers, run `npm run bench:transport-table`
([the rule](METHODOLOGY.md#default-transport-rule)).

STT speech end to final (`speech_end_to_final_ms`). It includes the vendor's end-of-speech wait
(VAD), so it is not the vendor's processing time alone; see the
[metric reference](METHODOLOGY.md#metric-reference).

| Vendor     | Transport | Cold          | Warm          |
| ---------- | --------- | ------------- | ------------- |
| ElevenLabs | websocket | 1443 / 1510   |               |
| Fish Audio | websocket | 1580.5 / 1662 |               |
| Groq       | http      | 1359 / 1412   | 1351.5 / 1366 |
| OpenAI     | http      | 1912 / 2570   | 1976 / 2433   |
| OpenAI     | websocket | 1670 / 1756   |               |

### End to end, 2026-10-20: [`results/2026-10-20-staging-eu/`](results/2026-10-20-staging-eu/)

A deployed counting-echo agent on VoiceThere's EU cluster, timed by one client at the speech
events. Conditions and what each folder ran: [`README.md`](results/2026-10-20-staging-eu/README.md).

VoiceThere speech (cluster STT and TTS, English), 1, 10 and 45 concurrent conversations. Source
table: [`cluster/summary.md`](results/2026-10-20-staging-eu/cluster/summary.md).

| Conversations | Phase | n   | `speech_end_to_final_ms` | `final_to_reply_start_ms` | `speech_end_to_reply_start_ms` | `tts_first_audio_ms` |
| ------------- | ----- | --- | ------------------------ | ------------------------- | ------------------------------ | -------------------- |
| 1             | cold  | 1   | 1485 / 1485              | 91 / 91                   | 1576 / 1576                    | 88 / 88              |
| 1             | warm  | 18  | 1597.5 / 1707            | 87.5 / 92                 | 1687 / 1797                    | 89 / 92              |
| 10            | cold  | 10  | 1562 / 1695              | 774 / 1273                | 2426.5 / 2806                  | 775.5 / 1274         |
| 10            | warm  | 181 | 1577 / 1725              | 186 / 383                 | 1786 / 2023                    | 188 / 385            |
| 45            | cold  | 45  | 1604 / 1763              | 124 / 313                 | 1748 / 1976                    | 117 / 312            |
| 45            | warm  | 768 | 1594 / 1736              | 132 / 336                 | 1766 / 1961                    | 129.5 / 330          |

Five vendor pairs on one conversation of 10 turns each, warm turns. Source table:
[`vendors/summary.md`](results/2026-10-20-staging-eu/vendors/summary.md).

| STT and TTS                             | n (warm) | Reply start after speech end | TTS first audio |
| --------------------------------------- | -------- | ---------------------------- | --------------- |
| Cluster STT, Cartesia `sonic-3`         | 9        | 1634 / 1921                  | 170 / 456       |
| Groq, Groq                              | 9        | 1777 / 1848                  | 180 / 181       |
| ElevenLabs Scribe v2, ElevenLabs        | 9        | 2424 / 2541                  | 663 / 700       |
| Fish Audio `transcribe-1`, Fish Audio   | 9        | 2219 / 2516                  | 273 / 319       |
| OpenAI `gpt-4o-mini-transcribe`, OpenAI | 9        | 3382 / 4398                  | 905 / 2040      |

"Reply start after speech end" is `speech_end_to_reply_start_ms`; "TTS first audio" is
`tts_first_audio_ms`.

### Not published

Deepgram and AssemblyAI: their terms do not allow benchmarking. Google, Azure and AWS: no account
yet. Details: [vendors in the published results](METHODOLOGY.md#vendors-in-the-published-results).

## Prerequisites

A Rust toolchain, Node 18 or newer, and an API key for each vendor you want to measure. A vendor
without keys is skipped (`--require` makes that a failure). The variable names per vendor are in
[`METHODOLOGY.md`](METHODOLOGY.md#reproduce).

## Run

```bash
npm run bench:vendors -- --vendors elevenlabs --runs 10 --region my-host --env-file <path>
```

`--env-file` takes whitespace-separated `NAME=value` tokens; the process environment works too.
Other flags: `--exclude`, `--transport`, `--out`, `--require`, `--force`. Keys are never printed
or written to a result file.

`--vendors all` measures the published set (ElevenLabs, Cartesia, Groq, OpenAI, Fish Audio); other
vendors run only when named. See [which vendors are published and why](METHODOLOGY.md#vendors-in-the-published-results).

## Output

Results go to `results/<date>-<region>/`:

- `raw.jsonl`: one record per sample.
- `summary.json` and `summary.md`: median, p90, min, max and n per group.
- `environment.json`: nwr version, git SHA, host, and DNS/TCP/TLS times to each vendor host.

## Compare two runs

```bash
npm run bench:compare -- benchmarks/results/<ours> benchmarks/results/<yours>
```

## More

- [`METHODOLOGY.md`](METHODOLOGY.md): inputs, cold and warm, what each metric starts and stops
  on, and what the numbers leave out.
- [`results/`](results/): published runs.
