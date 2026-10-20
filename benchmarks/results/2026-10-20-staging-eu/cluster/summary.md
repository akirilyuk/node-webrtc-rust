# End-to-end latency

## c1

- Date: 2026-10-20 (UTC)
- Source: e2e-load-staging
- Concurrency: 1, 1 conversations x 19 turns
- Started: 2026-10-20T00:58:45.509Z, finished: 2026-10-20T01:03:08.345Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere | webrtc | en/en | cold | counting-echo | 1 |  | 1485 / 1485 | 91 / 91 | 1576 / 1576 | 88 / 88 |
| voicethere | webrtc | en/en | warm | counting-echo | 18 |  | 1597.5 / 1707 | 87.5 / 92 | 1687 / 1797 | 89 / 92 |


## c10

- Date: 2026-10-19 (UTC)
- Source: e2e-load-staging
- Concurrency: 10, 10 conversations x 19.1 turns
- Started: 2026-10-19T19:39:43.885Z, finished: 2026-10-19T19:44:15.975Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere | webrtc | en/en | cold | counting-echo | 10 |  | 1562 / 1695 | 774 / 1273 | 2426.5 / 2806 | 775.5 / 1274 |
| voicethere | webrtc | en/en | warm | counting-echo | 181 |  | 1577 / 1725 | 186 / 383 | 1786 / 2023 | 188 / 385 |


## c45

- Date: 2026-10-19 (UTC)
- Source: e2e-load-staging
- Concurrency: 45, 45 conversations x 18.07 turns
- Started: 2026-10-19T19:44:42.098Z, finished: 2026-10-19T19:49:17.333Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere | webrtc | en/en | cold | counting-echo | 45 |  | 1604 / 1763 | 124 / 313 | 1748 / 1976 | 117 / 312 |
| voicethere | webrtc | en/en | warm | counting-echo | 768 |  | 1594 / 1736 | 132 / 336 | 1766 / 1961 | 129.5 / 330 |
