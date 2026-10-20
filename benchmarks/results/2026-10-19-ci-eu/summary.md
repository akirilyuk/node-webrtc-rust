# Vendor latency bench

- Date: 2026-10-19 (UTC)
- Region: ci-eu
- nwr 0.12.0 (unknown)
- Host: linux 6.8.0-100-generic x64, Intel(R) Core(TM) i7-7700 CPU @ 3.60GHz x8
- Runs: 10, transport: all

**Skipped:** deepgram (not published: vendor terms do not allow benchmarking); assemblyai (not published: vendor terms do not allow benchmarking); google (not published: no account yet); azure (not published: no account yet); aws (not published: no account yet)

### STT (median / p90)

| vendor | transport | model | phase | text | n | http | finalize_to_final_ms | finals | first_partial_ms | matched_words | open_ms | partials_before_finalize | pushed_ms | speech_end_to_final_ms | total_ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| elevenlabs | websocket | scribe_v2_realtime | cold | phrase | 10 | ws | 182.5 / 250 | 1 / 1 | 1869.5 / 1940 | 9 / 9 | 173.5 / 182 | 4 / 4 | 4180 / 4180 | 1443 / 1510 | 6647 / 6647 |
| fishaudio | websocket | transcribe-1 | cold | phrase | 10 | ws | 461 / 542 | 1 / 1 |  | 9 / 9 | 255.5 / 264 | 0 / 0 | 6060 / 6060 | 1580.5 / 1662 | 8733 / 8733 |
| groq | http | whisper-large-v3-turbo | cold | phrase | 10 | h2 | 239 / 292 | 1 / 1 |  | 9 / 9 | 0 / 0 | 0 / 0 | 4340 / 4340 | 1359 / 1412 | 7039 / 7039 |
| groq | http | whisper-large-v3-turbo | warm | phrase | 10 | h2 | 231 / 246 | 1 / 1 |  | 9 / 9 | 0 / 0 | 0 / 0 | 4340 / 4340 | 1351.5 / 1366 | 7039 / 7039 |
| openai | http | whisper-1 | cold | phrase | 10 | sdk | 852 / 1510 | 1 / 1 |  | 9 / 9 | 0 / 0 | 0 / 0 | 3900 / 3900 | 1912 / 2570 | 6625 / 6625 |
| openai | http | whisper-1 | warm | phrase | 10 | sdk | 916 / 1372 | 1 / 1 |  | 9 / 9 | 0 / 0 | 0 / 0 | 3900 / 3900 | 1976 / 2433 | 6625 / 6625 |
| openai | websocket | gpt-live-transcribe | cold | phrase | 10 | ws | 609.5 / 695 | 1 / 1 | 1100 / 1101 | 9 / 9 | 1095 / 1279 | 10 / 10 | 3900 / 3900 | 1670 / 1756 | 6625 / 6625 |

### TTS (median / p90)

| vendor | transport | model | phase | text | n | http | audio_ms | bytes | chunks | returned_bytes | rms | stall_ms | total_ms | ttfa_ms | real_time_factor |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| cartesia | http | sonic-3 | cold | sentence | 10 | h2 | 3347 / 3738 | 646442 / 722232 | 50 / 54 | 646442 / 722232 | 2723.0 / 3077.3 | 0 / 0 | 694 / 782 | 125 / 151 | 0.2 / 0.2 |
| cartesia | http | sonic-3 | cold | short | 10 | h2 | 1523.5 / 1710 | 294244 / 329908 | 22.5 / 25 | 294244 / 329908 | 2648.7 / 3798.6 | 0 / 0 | 423 / 464 | 152.5 / 211 | 0.3 / 0.4 |
| cartesia | http | sonic-3 | warm | sentence | 10 | h2 | 3440.5 / 3602 | 664278 / 695484 | 51.5 / 56 | 664278 / 695484 | 2707.4 / 2844.6 | 0 / 0 | 691.5 / 718 | 108.5 / 119 | 0.2 / 0.2 |
| cartesia | http | sonic-3 | warm | short | 10 | h2 | 1731 / 2081 | 334366 / 401240 | 25.5 / 30 | 334366 / 401240 | 2097.0 / 4074.2 | 0 / 0 | 416.5 / 475 | 129.5 / 142 | 0.2 / 0.3 |
| cartesia | websocket | sonic-3 | cold | sentence | 10 | ws | 3223 / 3525 | 619694 / 677652 | 18 / 21 | 619694 / 677652 | 2895.6 / 3145.0 | 0 / 0 | 766 / 900 | 228 / 267 | 0.2 / 0.3 |
| cartesia | websocket | sonic-3 | cold | short | 10 | ws | 1391.5 / 1810 | 267494 / 347744 | 9 / 11 | 267494 / 347744 | 2700.9 / 3240.8 | 0 / 0 | 472.5 / 553 | 268 / 281 | 0.3 / 0.4 |
| cartesia | websocket | sonic-3 | warm | sentence | 10 | ws | 3177.5 / 3479 | 610778 / 668736 | 18.5 / 20 | 610778 / 668736 | 2871.6 / 3141.8 | 0 / 0 | 661 / 695 | 106 / 109 | 0.2 / 0.2 |
| cartesia | websocket | sonic-3 | warm | short | 10 | ws | 1785 / 2830 | 343284 / 543904 | 11 / 17 | 343284 / 543904 | 2314.4 / 3209.7 | 0 / 0 | 482 / 610 | 128 / 141 | 0.3 / 0.3 |
| elevenlabs | http | eleven_multilingual_v2 | cold | sentence | 10 | h2 | 3765 / 3883 | 726690 / 748984 | 47.5 / 75 | 726690 / 748984 | 4492.2 / 4734.3 | 0 / 0 | 822.5 / 830 | 755 / 765 | 0.2 / 0.2 |
| elevenlabs | http | eleven_multilingual_v2 | cold | short | 10 | h2 | 1246.5 / 1336 | 240744 / 258576 | 17 / 24 | 240744 / 258576 | 5164.1 / 5633.9 | 0 / 0 | 645 / 661 | 606.5 / 625 | 0.5 / 0.6 |
| elevenlabs | http | eleven_multilingual_v2 | warm | sentence | 10 | h2 | 3945.5 / 4014 | 762358 / 775732 | 47.5 / 58 | 762358 / 775732 | 4355.0 / 4957.3 | 0 / 0 | 789.5 / 813 | 727.5 / 763 | 0.2 / 0.2 |
| elevenlabs | http | eleven_multilingual_v2 | warm | short | 10 | h2 | 1270.5 / 1341 | 245202 / 258576 | 14 / 25 | 245202 / 258576 | 4511.6 / 5715.9 | 0 / 0 | 613 / 635 | 585.5 / 597 | 0.5 / 0.5 |
| elevenlabs | websocket | eleven_multilingual_v2 | cold | sentence | 10 | ws | 3853 / 3993 | 740068 / 766816 | 2 / 2 | 740068 / 766816 | 4553.6 / 4888.7 | 0 / 0 | 526.5 / 571 | 511 / 555 | 0.1 / 0.1 |
| elevenlabs | websocket | eleven_multilingual_v2 | cold | short | 10 | ws | 1229.5 / 1392 | 236286 / 267492 | 2 / 2 | 236286 / 267492 | 4196.8 / 5568.2 | 0 / 0 | 348 / 384 | 340 / 374 | 0.3 / 0.3 |
| elevenlabs | websocket | eleven_multilingual_v2 | warm | sentence | 10 | ws | 3783.5 / 3993 | 726690 / 766816 | 2 / 2 | 726690 / 766816 | 4604.0 / 5048.2 | 0 / 0 | 520 / 529 | 504 / 514 | 0.1 / 0.1 |
| elevenlabs | websocket | eleven_multilingual_v2 | warm | short | 10 | ws | 1299 / 1392 | 249660 / 267492 | 2 / 2 | 249660 / 267492 | 5091.2 / 5576.8 | 0 / 0 | 373.5 / 391 | 363 / 380 | 0.3 / 0.3 |
| fishaudio | http | drama-3 | cold | sentence | 10 | h2 | 4067 / 5226 | 784084 / 1006992 | 30 / 35 | 784084 / 1006992 | 3718.8 / 7496.4 | 0 / 0 | 1007.5 / 1186 | 319 / 361 | 0.2 / 0.3 |
| fishaudio | http | drama-3 | cold | short | 10 | h2 | 1595 / 1803 | 307052 / 347176 | 12 / 13 | 307052 / 347176 | 3973.3 / 4679.0 | 0 / 0 | 558 / 871 | 334.5 / 352 | 0.4 / 0.6 |
| fishaudio | http | drama-3 | warm | sentence | 10 | h2 | 4254.5 / 4577 | 819748 / 882160 | 29 / 33 | 819748 / 882160 | 3778.2 / 5912.7 | 0 / 0 | 981.5 / 1129 | 291.5 / 317 | 0.2 / 0.3 |
| fishaudio | http | drama-3 | warm | short | 10 | h2 | 1502.5 / 1757 | 289220 / 338256 | 11 / 13 | 289220 / 338256 | 3230.6 / 5483.9 | 0 / 0 | 557 / 812 | 290.5 / 304 | 0.3 / 0.6 |
| fishaudio | websocket | drama-3 | cold | sentence | 10 | ws | 4362 / 4640 | 837584 / 891080 | 6 / 6 | 837584 / 891080 | 4436.4 / 5775.6 | 0 / 0 | 1588.5 / 1654 | 1023 / 1065 | 0.4 / 0.4 |
| fishaudio | websocket | drama-3 | cold | short | 10 | ws | 1436 / 1808 | 275844 / 347176 | 4.5 / 5 | 275844 / 347176 | 4345.2 / 6017.2 | 0 / 0 | 1213 / 1325 | 1033 / 1075 | 0.9 / 1.0 |
| fishaudio | websocket | drama-3 | warm | sentence | 10 | ws | 4176 / 4408 | 801912 / 846496 | 6 / 6 | 801912 / 846496 | 4208.4 / 5790.6 | 0 / 0 | 1546 / 1645 | 1023.5 / 1073 | 0.4 / 0.4 |
| fishaudio | websocket | drama-3 | warm | short | 10 | ws | 1436 / 1529 | 275848 / 293680 | 4 / 5 | 275848 / 293680 | 2627.8 / 4774.9 | 0 / 0 | 1229.5 / 1315 | 1021 / 1124 | 0.9 / 1.1 |
| groq | http | canopylabs/orpheus-v1-english | cold | sentence | 10 | h2 | 3506 / 3506 | 675808 / 675808 | 30 / 31 | 675808 / 675808 | 3252.0 / 3252.0 | 0 / 0 | 681 / 688 | 229.5 / 239 | 0.2 / 0.2 |
| groq | http | canopylabs/orpheus-v1-english | cold | short | 10 | h2 | 2152 / 2152 | 414688 / 414688 | 19 / 20 | 414688 / 414688 | 2656.5 / 2656.5 | 0 / 0 | 472.5 / 488 | 219 / 238 | 0.2 / 0.2 |
| groq | http | canopylabs/orpheus-v1-english | warm | sentence | 10 | h2 | 3506 / 3506 | 675808 / 675808 | 30 / 31 | 675808 / 675808 | 3252.0 / 3252.0 | 0 / 0 | 648.5 / 662 | 198 / 213 | 0.2 / 0.2 |
| groq | http | canopylabs/orpheus-v1-english | warm | short | 10 | h2 | 2152 / 2152 | 414688 / 414688 | 19 / 20 | 414688 / 414688 | 2656.5 / 2656.5 | 0 / 0 | 438 / 455 | 194 / 205 | 0.2 / 0.2 |
| openai | http | tts-1 | cold | sentence | 10 | h2 | 4118 / 4655 | 794400 / 897600 | 35.5 / 41 | 794400 / 897600 | 2135.8 / 2161.1 | 51.5 / 130 | 1815 / 2699 | 1184.5 / 1614 | 0.4 / 0.7 |
| openai | http | tts-1 | cold | short | 10 | h2 | 915 / 946 | 176400 / 182400 | 9.5 / 11 | 176400 / 182400 | 3338.8 / 3378.1 | 20 / 45 | 1340.5 / 1766 | 1166 / 1651 | 1.5 / 1.9 |
| openai | http | tts-1 | warm | sentence | 10 | h2 | 4063 / 4515 | 783600 / 871200 | 36 / 39 | 783600 / 871200 | 2137.3 / 2170.9 | 5 / 47 | 1428 / 1814 | 1004 / 1250 | 0.4 / 0.5 |
| openai | http | tts-1 | warm | short | 10 | h2 | 933 / 971 | 180000 / 187200 | 10 / 11 | 180000 / 187200 | 3320.0 / 3390.2 | 0 / 52 | 1018 / 1445 | 824.5 / 1263 | 1.1 / 1.5 |

