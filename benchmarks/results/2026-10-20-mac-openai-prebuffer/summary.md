# Vendor latency bench

- Date: 2026-10-20 (UTC)
- Region: mac-prebuffer
- nwr 0.12.1 (ad5314f2a)
- Host: darwin 24.6.0 arm64, Apple M4 Pro x14
- Runs: 10, transport: all

### STT (median / p90)

| vendor | transport | model | phase | text | n | http | finalize_to_final_ms | finals | matched_words | open_ms | partials_before_finalize | pushed_ms | speech_end_to_final_ms | total_ms | first_partial_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| openai | http | whisper-1 | cold | phrase | 10 | sdk | 1844.5 / 2226 | 1 / 1 | 9 / 9 | 0 / 0 | 0 / 0 | 3980 / 3980 | 2825.5 / 3208 | 6712 / 6712 |  |
| openai | http | whisper-1 | warm | phrase | 10 | sdk | 1312.5 / 2520 | 1 / 1 | 9 / 9 | 0 / 0 | 0 / 0 | 3980 / 3980 | 2292.5 / 3499 | 6712 / 6712 |  |
| openai | websocket | gpt-live-transcribe | cold | phrase | 10 | ws | 637.5 / 793 | 1 / 1 | 9 / 9 | 1646.5 / 2093 | 10 / 10 | 3980 / 3980 | 1618 / 1772 | 6712 / 6712 | 1280 / 1360 |

### TTS (median / p90)

| vendor | transport | model | phase | text | n | http | audio_ms | bytes | chunks | playout_start_ms | returned_bytes | rms | stall_ms | total_ms | ttfa_ms | real_time_factor |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| openai | http | tts-1 | cold | sentence | 10 | h2 | 4237.5 / 4653 | 817200 / 897600 | 35 / 38 | 1503.5 / 2451 | 817200 / 897600 | 2132.3 / 2154.4 | 0 / 0 | 2172.5 / 2943 | 1252.5 / 2077 | 0.5 / 0.7 |
| openai | http | tts-1 | cold | short | 10 | h2 | 933 / 958 | 180000 / 184800 | 10 / 11 | 1271 / 2586 | 180000 / 184800 | 3330.7 / 3416.5 | 0 / 0 | 1497 / 2610 | 1022 / 2246 | 1.6 / 2.8 |
| openai | http | tts-1 | warm | sentence | 10 | h2 | 4138.5 / 4542 | 798000 / 876000 | 37 / 39 | 1838.5 / 2162 | 798000 / 876000 | 2135.9 / 2160.3 | 0 / 0 | 2234 / 2573 | 1587 / 1835 | 0.6 / 0.6 |
| openai | http | tts-1 | warm | short | 10 | h2 | 933.5 / 958 | 180000 / 184800 | 10 / 11 | 1292.5 / 2128 | 180000 / 184800 | 3320.1 / 3367.2 | 0 / 0 | 1446.5 / 2181 | 1059 / 1774 | 1.6 / 2.3 |

