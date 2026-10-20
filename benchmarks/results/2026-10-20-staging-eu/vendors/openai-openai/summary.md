# End-to-end latency

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:27:48.309Z, finished: 2026-10-20T01:30:41.056Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+openai/openai | webrtc | gpt-4o-mini-transcribe/tts-1 | cold | counting-echo | 1 |  | 2534 / 2534 | 1057 / 1057 | 3591 / 3591 | 1260 / 1260 |
| voicethere+openai/openai | webrtc | gpt-4o-mini-transcribe/tts-1 | warm | counting-echo | 9 |  | 2316 / 3695 | 872 / 1833 | 3382 / 4398 | 905 / 2040 |

