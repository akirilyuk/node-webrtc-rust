# 2026-10-19, region `ci-eu`

Vendor latency results for nwr 0.12.0. The four files next to this one are the unedited output
of the bench CLI. How the numbers are measured and read: [`METHODOLOGY.md`](../../METHODOLOGY.md).

## Source

- Workflow run 39304540265 of `vendor-bench.yml` on pull request #479, head commit
  `0a7cd5011e747798b1dce56b52e3ccd3d6963fbd`.
- `environment.json` has `git_sha: null` because the CI container could not run `git`. The CLI
  now takes the SHA from `NWR_BENCH_GIT_SHA` or `GITHUB_SHA` in that case (`git_sha_source`).

## Where it ran

A self-hosted CI runner in a Hetzner data centre in the EU (Intel Core i7-7700, 8 threads, 62.6 GB
memory), in the same data-centre network as VoiceThere's agent runners. DNS, TCP and TLS times to
each vendor host are in `environment.json`.

## What ran

- Vendors run: ElevenLabs, Cartesia, Groq, OpenAI, Fish Audio.
- Not published: Deepgram and AssemblyAI (vendor terms do not allow benchmarking), Google, Azure
  and AWS (no account yet).
- 10 runs per group, `--transport all`.
- Groq was paced at one request every 6500 ms to stay under its documented rate limit.
- Idle wait: every vendor window started after the host reached an idle CPU (`reached: true`).
- Nothing is inconclusive: no starved vendor, no failed vendor, no host suspend.
