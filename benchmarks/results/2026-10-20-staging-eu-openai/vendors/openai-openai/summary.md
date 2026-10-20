# End-to-end latency

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T08:30:55.914Z, finished: 2026-10-20T08:33:54.015Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+openai/openai | webrtc | gpt-4o-mini-transcribe/tts-1 | cold | counting-echo | 1 |  | 2600 / 2600 | 2150 / 2150 | 4750 / 4750 | 2345 / 2345 |
| voicethere+openai/openai | webrtc | gpt-4o-mini-transcribe/tts-1 | warm | counting-echo | 9 |  | 2250 / 3859 | 1290 / 1843 | 3595 / 5506 | 1303 / 1843 |

