# End-to-end results, VoiceThere staging (EU), 2026-10-19/20

Turn latency of a deployed counting-echo agent on VoiceThere's EU staging cluster, measured by a
client that speaks a counting phrase and times the server's speech events. Metric definitions,
cold and warm, and `meta.json` fields: [End-to-end results](../../METHODOLOGY.md#end-to-end-results-voicethere).

## What ran

| Folder | What | Turns | When (UTC) |
| --- | --- | --- | --- |
| `cluster/c1` | VoiceThere speech (cluster STT/TTS, English), 1 conversation | 19 | 2026-10-20 00:57–01:03 |
| `cluster/c10` | same, 10 concurrent conversations | 191 | 2026-10-19 19:39–19:44 |
| `cluster/c45` | same, 45 concurrent conversations | 813 | 2026-10-19 19:44–19:49 |
| `vendors/sherpa-cartesia` | cluster STT, Cartesia `sonic-3` TTS, 1 conversation | 10 | 2026-10-20 01:06–01:19 |
| `vendors/groq-groq` | Groq STT and TTS | 10 | 2026-10-20 01:19–01:26 |
| `vendors/openai-openai` | OpenAI `gpt-4o-mini-transcribe` STT, `tts-1` TTS | 10 | 2026-10-20 01:26–01:32 |
| `vendors/elevenlabs-elevenlabs` | ElevenLabs Scribe v2 realtime STT, `eleven_multilingual_v2` TTS | 10 | 2026-10-20 01:44–01:51 |
| `vendors/fishaudio-fishaudio` | Fish Audio `transcribe-1` STT, `drama-3` TTS | 10 | 2026-10-20 01:51–01:58 |

`cluster/summary.md` and `vendors/summary.md` collect the per-folder summaries.

## Conditions

- Agent runners on node-webrtc-rust 0.12.0, so ElevenLabs TTS used HTTP and Cartesia TTS used
  WebSocket (the defaults before the 0.12.1 change; see [Default transport rule](../../METHODOLOGY.md#default-transport-rule)).
- The runner image changed once between the first and the later runs; the change only affected
  how transcript rows are timestamped, not the speech path. A 1-conversation run that overlapped
  that rollout was discarded and `cluster/c1` was run again.
- Every run had the cluster to itself: scheduled test suites and probes were paused, speech pools
  autoscaled as for a customer (nothing pinned), and the TTS phrase cache was off, so every reply
  was synthesised.
- The client was one laptop (see `environment.json`). Its network path to the EU region is part of
  every number except `tts_first_audio_ms`, which the agent runner measures. No host suspend and
  no client starvation was recorded (`meta.json`).
- The cloud-vendor runs check the reply on the client in two ways: the reply audio must arrive
  (fatal if not), and a local transcription of it is recorded per turn (`inbound_echo_ok`). The
  local transcriber split some Fish Audio and ElevenLabs replies at a pause (9/10 and 8/10 turns
  confirmed); the server-side transcript matched in every turn.

## Not here

Deepgram and AssemblyAI: their terms do not allow benchmarking, so they were not run. Google
Cloud, Azure and AWS: not measured yet. See
[Vendors in the published results](../../METHODOLOGY.md#vendors-in-the-published-results).
