# End-to-end latency

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:20:33.989Z, finished: 2026-10-20T01:25:23.800Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+groq/groq | webrtc | whisper-large-v3-turbo/canopylabs/orpheus-v1-english | cold | counting-echo | 1 |  | 1603 / 1603 | 183 / 183 | 1786 / 1786 | 182 / 182 |
| voicethere+groq/groq | webrtc | whisper-large-v3-turbo/canopylabs/orpheus-v1-english | warm | counting-echo | 9 |  | 1596 / 1671 | 181 / 183 | 1777 / 1848 | 180 / 181 |

