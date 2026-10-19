# Benchmarks

Latency of the cloud speech vendors (TTS time to first audio, STT open, first partial and final),
measured against the vendors' real APIs by the vendor crates of this library. The CLI records
where it ran, so a run on your machine can be compared with ours.

## Prerequisites

A Rust toolchain, Node 18 or newer, and an API key for each vendor you want to measure. A vendor
without keys is skipped (`--require` makes that a failure). The variable names per vendor are in
[`METHODOLOGY.md`](METHODOLOGY.md#reproduce).

## Run

```bash
npm run bench:vendors -- --vendors deepgram --runs 5 --region my-host --env-file <path>
```

`--env-file` takes whitespace-separated `NAME=value` tokens; the process environment works too.
Other flags: `--exclude`, `--transport`, `--out`, `--require`, `--force`. Keys are never printed
or written to a result file.

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
