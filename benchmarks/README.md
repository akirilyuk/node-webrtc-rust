# Benchmarks

Reproducible latency measurements of the cloud speech vendors (TTS time to first audio, STT open,
partial and final latency), run against the vendors' real APIs. The CLI wraps the Rust
`crates/vendor-<id>/tests/live_bench.rs` benches and records where it ran, so results from
different machines and regions can be compared.

`METHODOLOGY.md` follows.

## Run

```bash
npm run bench:vendors -- --vendors deepgram --runs 3 --region my-host --env-file <path>
```

Without keys a vendor is skipped (add `--require` to fail instead). Results go to
`benchmarks/results/<date>-<region>/`: `raw.jsonl`, `summary.json`, `summary.md`,
`environment.json`. Flags: `--vendors <list|all>`, `--exclude <list>`, `--runs <n>`,
`--transport <all|websocket|http|auto>`, `--region <label>`, `--out <dir>`, `--env-file <path>`,
`--require`, `--force`.

Keys come from the process environment or from `--env-file` (whitespace-separated `NAME=value`
tokens). The library names:

| Vendor     | Variables                                                                      |
| ---------- | ------------------------------------------------------------------------------ |
| deepgram   | `DEEPGRAM_API_KEY`                                                             |
| elevenlabs | `ELEVENLABS_API_KEY`                                                           |
| cartesia   | `CARTESIA_API_KEY`, `CARTESIA_VOICE_ID`                                        |
| assemblyai | `ASSEMBLYAI_API_KEY`                                                           |
| google     | `GOOGLE_API_KEY`, or `GOOGLE_APPLICATION_CREDENTIALS` + `GOOGLE_CLOUD_PROJECT` |
| groq       | `GROQ_API_KEY`                                                                 |
| azure      | `AZURE_SPEECH_KEY`, `AZURE_SPEECH_REGION`                                      |
| aws        | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`                     |
| openai     | `OPENAI_API_KEY`                                                               |
| fishaudio  | `FISH_API_KEY`                                                                 |

A key value is never printed or written to a result file.

## Compare two runs

```bash
npm run bench:compare -- benchmarks/results/<a> benchmarks/results/<b>
```
