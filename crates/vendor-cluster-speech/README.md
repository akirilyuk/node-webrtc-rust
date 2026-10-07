# node-webrtc-rust-vendor-cluster-speech

In-cluster gRPC STT/TTS vendor (`cluster-sherpa`). STT uses the bidirectional `Transcribe` stream of
the speech service; TTS uses `Synthesize`.

## Environment

| Variable                           | Default                          | Effect                                                                                                                                                                                                                                                                                                                             |
| ---------------------------------- | -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CLUSTER_STT_STREAM_PER_UTTERANCE` | off (`1`/`true`/`yes`/`on` = on) | Open one `Transcribe` stream per utterance instead of one for the whole session. The stream closes after each `Finalized` and re-opens on the next audio (denied opens retry every 200 ms), so the load balancer and the pods' admission cap place every turn. Read once per process; `ClusterSttOptions` overrides it per client. |

Metrics (in-process counters, see `metrics.rs`): `cluster_stt_utterance_streams_total` (per-utterance
opens) and `cluster_stt_stream_open_ms` (open-to-`Ready` latency, attribute `reason`:
`session_start` | `utterance` | `reopen`).
