# End-to-end latency

- Date: 2026-10-19 (UTC)
- Source: e2e-load-staging
- Concurrency: 45, 45 conversations x 18.07 turns
- Started: 2026-10-19T19:44:42.098Z, finished: 2026-10-19T19:49:17.333Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere | webrtc | en/en | cold | counting-echo | 45 |  | 1604 / 1763 | 124 / 313 | 1748 / 1976 | 117 / 312 |
| voicethere | webrtc | en/en | warm | counting-echo | 768 |  | 1594 / 1736 | 132 / 336 | 1766 / 1961 | 129.5 / 330 |

