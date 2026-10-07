# node-webrtc-rust-vendor-cluster-speech

In-cluster gRPC STT/TTS vendor (`cluster-sherpa`). STT uses the bidirectional `Transcribe` stream of
the speech service; TTS uses `Synthesize`.

## Environment

| Variable                           | Default                          | Effect                                                                                                                                                                                                                                                                                                                             |
| ---------------------------------- | -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CLUSTER_STT_STREAM_PER_UTTERANCE` | off (`1`/`true`/`yes`/`on` = on) | Open one `Transcribe` stream per utterance instead of one for the whole session. The stream closes after each `Finalized` and re-opens on the next audio (denied opens retry every 200 ms), so the load balancer and the pods' admission cap place every turn. Read once per process; `ClusterSttOptions` overrides it per client. |

| `SPEECH_STT_OPEN_WAIT_MAX_MS` | `120000` | How long `finalize_utterance` waits for a refused `Transcribe` open (a speech pod at its stream cap) to succeed before it sends the Finalize anyway. Audio stays queued meanwhile and `SttProvider::stream_open_pending()` reports `true`, so VoiceAgent holds the turn instead of ending it with `user_stt_not_found` (cap `vad.sttOpenWaitMaxMs`). |

| `SPEECH_TTS_OPEN_WAIT_MAX_MS` | `30000` | How long a `Synthesize` start refused with `UNAVAILABLE` or `RESOURCE_EXHAUSTED` is retried (every 200 ms) before the synthesis fails; `0` = fail at the first refusal. Only refusals before any audio are retried. Each synthesis that waited emits the `tts_wait` speech event (`waitMs`, `attempts`, `reason`). |

Metrics (in-process counters, see `metrics.rs`): `cluster_stt_utterance_streams_total` (per-utterance
opens) and `cluster_stt_stream_open_ms` (open-to-`Ready` latency, attribute `reason`:
`session_start` | `utterance` | `reopen`). TTS: `cluster_tts_open_retries_total` (refused starts
that were retried) and `cluster_tts_open_wait_ms` (wait of each synthesis that needed a retry).
