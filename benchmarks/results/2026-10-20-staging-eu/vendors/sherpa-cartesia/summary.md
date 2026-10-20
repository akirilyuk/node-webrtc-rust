# End-to-end latency

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:07:53.427Z, finished: 2026-10-20T01:11:20.412Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+sherpa/cartesia | webrtc | en/sonic-3 | cold | counting-echo | 1 |  | 1454 / 1454 | 168 / 168 | 1622 / 1622 | 166 / 166 |
| voicethere+sherpa/cartesia | webrtc | en/sonic-3 | warm | counting-echo | 9 |  | 1465 / 1858 | 169 / 456 | 1634 / 1921 | 170 / 456 |

