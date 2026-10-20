# End-to-end latency

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:52:17.602Z, finished: 2026-10-20T01:57:28.695Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+fishaudio/fishaudio | webrtc | transcribe-1/drama-3 | cold | counting-echo | 1 |  | 2255 / 2255 | 283 / 283 | 2538 / 2538 | 279 / 279 |
| voicethere+fishaudio/fishaudio | webrtc | transcribe-1/drama-3 | warm | counting-echo | 9 |  | 1962 / 2233 | 274 / 321 | 2219 / 2516 | 273 / 319 |

