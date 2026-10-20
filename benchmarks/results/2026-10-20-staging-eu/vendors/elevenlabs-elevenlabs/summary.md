# End-to-end latency

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:45:27.539Z, finished: 2026-10-20T01:49:39.654Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+elevenlabs/elevenlabs | webrtc | scribe_v2_realtime/eleven_multilingual_v2 | cold | counting-echo | 1 |  | 1874 / 1874 | 682 / 682 | 2556 / 2556 | 677 / 677 |
| voicethere+elevenlabs/elevenlabs | webrtc | scribe_v2_realtime/eleven_multilingual_v2 | warm | counting-echo | 9 |  | 1765 / 1862 | 666 / 702 | 2424 / 2541 | 663 / 700 |

