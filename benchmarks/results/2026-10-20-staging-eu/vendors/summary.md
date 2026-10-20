# End-to-end latency

## elevenlabs-elevenlabs

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:45:27.539Z, finished: 2026-10-20T01:49:39.654Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+elevenlabs/elevenlabs | webrtc | scribe_v2_realtime/eleven_multilingual_v2 | cold | counting-echo | 1 |  | 1874 / 1874 | 682 / 682 | 2556 / 2556 | 677 / 677 |
| voicethere+elevenlabs/elevenlabs | webrtc | scribe_v2_realtime/eleven_multilingual_v2 | warm | counting-echo | 9 |  | 1765 / 1862 | 666 / 702 | 2424 / 2541 | 663 / 700 |


## fishaudio-fishaudio

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:52:17.602Z, finished: 2026-10-20T01:57:28.695Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+fishaudio/fishaudio | webrtc | transcribe-1/drama-3 | cold | counting-echo | 1 |  | 2255 / 2255 | 283 / 283 | 2538 / 2538 | 279 / 279 |
| voicethere+fishaudio/fishaudio | webrtc | transcribe-1/drama-3 | warm | counting-echo | 9 |  | 1962 / 2233 | 274 / 321 | 2219 / 2516 | 273 / 319 |


## groq-groq

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:20:33.989Z, finished: 2026-10-20T01:25:23.800Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+groq/groq | webrtc | whisper-large-v3-turbo/canopylabs/orpheus-v1-english | cold | counting-echo | 1 |  | 1603 / 1603 | 183 / 183 | 1786 / 1786 | 182 / 182 |
| voicethere+groq/groq | webrtc | whisper-large-v3-turbo/canopylabs/orpheus-v1-english | warm | counting-echo | 9 |  | 1596 / 1671 | 181 / 183 | 1777 / 1848 | 180 / 181 |


## openai-openai

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:27:48.309Z, finished: 2026-10-20T01:30:41.056Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+openai/openai | webrtc | gpt-4o-mini-transcribe/tts-1 | cold | counting-echo | 1 |  | 2534 / 2534 | 1057 / 1057 | 3591 / 3591 | 1260 / 1260 |
| voicethere+openai/openai | webrtc | gpt-4o-mini-transcribe/tts-1 | warm | counting-echo | 9 |  | 2316 / 3695 | 872 / 1833 | 3382 / 4398 | 905 / 2040 |


## sherpa-cartesia

- Date: 2026-10-20 (UTC)
- Source: e2e-cloud-vendor-smoke
- Concurrency: 1, 1 conversations x 10 turns
- Started: 2026-10-20T01:07:53.427Z, finished: 2026-10-20T01:11:20.412Z

### END-TO-END (median / p90)

| vendor | transport | model | phase | text | n | http | speech_end_to_final_ms | final_to_reply_start_ms | speech_end_to_reply_start_ms | tts_first_audio_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| voicethere+sherpa/cartesia | webrtc | en/sonic-3 | cold | counting-echo | 1 |  | 1454 / 1454 | 168 / 168 | 1622 / 1622 | 166 / 166 |
| voicethere+sherpa/cartesia | webrtc | en/sonic-3 | warm | counting-echo | 9 |  | 1465 / 1858 | 169 / 456 | 1634 / 1921 | 170 / 456 |
