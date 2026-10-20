# End-to-end latency

- Date: 2026-10-19 (UTC)
- Source: e2e-load-staging
- Concurrency: 10, 10 conversations x 19.1 turns
- Started: 2026-10-19T19:39:43.885Z, finished: 2026-10-19T19:44:15.975Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere | webrtc | en/en | cold | counting-echo | 10 |  | 1562 / 1695 | 774 / 1273 | 2426.5 / 2806 | 775.5 / 1274 |
| voicethere | webrtc | en/en | warm | counting-echo | 181 |  | 1577 / 1725 | 186 / 383 | 1786 / 2023 | 188 / 385 |

