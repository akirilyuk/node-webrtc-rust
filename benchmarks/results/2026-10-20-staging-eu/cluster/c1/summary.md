# End-to-end latency

- Date: 2026-10-20 (UTC)
- Source: e2e-load-staging
- Concurrency: 1, 1 conversations x 19 turns
- Started: 2026-10-20T00:58:45.509Z, finished: 2026-10-20T01:03:08.345Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere | webrtc | en/en | cold | counting-echo | 1 |  | 1485 / 1485 | 91 / 91 | 1576 / 1576 | 88 / 88 |
| voicethere | webrtc | en/en | warm | counting-echo | 18 |  | 1597.5 / 1707 | 87.5 / 92 | 1687 / 1797 | 89 / 92 |

