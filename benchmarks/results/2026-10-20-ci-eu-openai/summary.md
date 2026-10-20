# Vendor latency bench

- Date: 2026-10-20 (UTC)
- Region: ci-eu
- nwr 0.12.2 (0c7213ad5)
- Host: linux 6.8.0-100-generic x64, Intel(R) Core(TM) i7-7700 CPU @ 3.60GHz x8
- Runs: 10, transport: all

### STT (median / p90)

| vendor | transport | model | phase | text | n | http | finalize_to_final_ms | finals | matched_words | open_ms | partials_before_finalize | pushed_ms | speech_end_to_final_ms | total_ms | first_partial_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| openai | http | whisper-1 | cold | phrase | 10 | sdk | 901.5 / 1344 | 1 / 1 | 9 / 9 | 0 / 0 | 0 / 0 | 3960 / 3960 | 1842 / 2284 | 6750 / 6750 |  |
| openai | http | whisper-1 | warm | phrase | 10 | sdk | 887 / 1317 | 1 / 1 | 9 / 9 | 0 / 0 | 0 / 0 | 3960 / 3960 | 1826.5 / 2258 | 6750 / 6750 |  |
| openai | websocket | gpt-live-transcribe | cold | phrase | 10 | ws | 602 / 697 | 1 / 1 | 9 / 9 | 1168 / 1308 | 10 / 10 | 3960 / 3960 | 1541.5 / 1637 | 6750 / 6750 | 1159.5 / 1260 |

### TTS (median / p90)

| vendor | transport | model | phase | text | n | http | audio_ms | bytes | chunks | playout_start_ms | returned_bytes | rms | stall_ms | total_ms | ttfa_ms | real_time_factor |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| openai | http | tts-1 | cold | sentence | 10 | h2 | 4406 / 4665 | 849600 / 900000 | 35.5 / 42 | 1253.5 / 1554 | 849600 / 900000 | 2117.3 / 2147.8 | 0 / 0 | 1486 / 1788 | 1095 / 1377 | 0.4 / 0.4 |
| openai | http | tts-1 | cold | short | 10 | h2 | 933 / 983 | 180000 / 189600 | 10 / 12 | 1518 / 1650 | 180000 / 189600 | 3323.6 / 3348.9 | 0 / 0 | 1560.5 / 1842 | 1352 / 1613 | 1.7 / 2 |
| openai | http | tts-1 | warm | sentence | 10 | h2 | 4150 / 4468 | 800400 / 861600 | 34 / 41 | 1268.5 / 1524 | 800400 / 861600 | 2132.6 / 2166.9 | 0 / 0 | 1502 / 1821 | 1126 / 1355 | 0.4 / 0.4 |
| openai | http | tts-1 | warm | short | 10 | h2 | 933.5 / 983 | 180000 / 189600 | 10.5 / 11 | 1414.5 / 2073 | 180000 / 189600 | 3334.2 / 3404.0 | 0 / 0 | 1471 / 2207 | 1226.5 / 1884 | 1.6 / 2.3 |

