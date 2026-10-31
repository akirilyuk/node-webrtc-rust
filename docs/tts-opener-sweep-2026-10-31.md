# TTS opener sweep (2026-10-31)

Lone short openers ("Hi!") at the start of a Sherpa TTS reply, as written and merged into the next sentence.

## Result

- **Quiet opener** (some opener of at most 4 letters, as written, has median `delta_db < -10` or STT misses its first word in the majority of 3 syntheses), at either speed: 41 of 42 English models: `en`, `en-amy-low`, `en-lessac`, `en-lessac-high`, `en-lessac-low`, `en-glados`, `en-glados-high`, `en-ryan`, `en-ryan-high`, `en-ryan-low`, `en-joe`, `en-bryce`, `en-john`, `en-kristin`, `en-norman`, `en-danny-low`, `en-kathleen-low`, `en-ljspeech`, `en-ljspeech-high`, `en-arctic`, `en-hfc-female`, `en-hfc-male`, `en-gb-alan`, `en-gb-cori`, `en-gb-jenny`, `en-kitten-f2`, `en-kitten-m2`, `en-kitten-f3`, `en-kitten-m3`, `en-kitten-f4`, `en-kitten-m4`, `en-kitten-f5`, `en-kitten-m5`, `en-kokoro-heart`, `en-kokoro-nicole`, `en-kokoro-sarah`, `en-kokoro-michael`, `en-kokoro-fenrir`, `en-kokoro-puck`, `en-gb-kokoro-emma`, `en-gb-kokoro-george`.
- **Merged quieter than written** (median `delta_db` merged < written): 687 of 1680 cells. Worst: `en-kokoro-nicole` 1.1 Yep! 2.9 → -8.5 (-11.4 dB); `en-kokoro-nicole` 1.1 Got it! 2.6 → -6.4 (-9.1 dB); `en-kokoro-nicole` 1.1 OK! 3.4 → -4.8 (-8.1 dB); `en-kokoro-nicole` 1 Got it! 3.2 → -4.3 (-7.6 dB); `en-danny-low` 1 Thanks! 1.0 → -6.6 (-7.5 dB); `en-kokoro-nicole` 1.1 Okay! 3.3 → -3.6 (-7.0 dB); `en-danny-low` 1.1 Okay! 3.9 → -2.8 (-6.7 dB); `en-kokoro-nicole` 1 OK! 4.1 → -2.4 (-6.5 dB); `en-kokoro-nicole` 1 Yes! 4.0 → -2.3 (-6.2 dB); `en-kokoro-nicole` 1.1 Thanks! 3.5 → -2.7 (-6.2 dB).
- **Merged loses the first word** (written STT hit by majority, merged not): 104 cells: `en-lessac` 1 Done! (3/3 → 1/3, merged heard 'Done ask me anything about voice there.'); `en-lessac` 1 Got it! (2/3 → 1/3, merged heard 'Guard it, ask me anything about voice there.'); `en-lessac` 1.1 Wow! (3/3 → 0/3, merged heard 'Well, ask me anything about voice there.'); `en-lessac` 1.1 Got it! (2/3 → 0/3, merged heard 'Hot it. Ask me anything about voice there.'); `en-lessac-high` 1 Cool! (3/3 → 1/3, merged heard 'School asked me anything about voice there.'); `en-lessac-high` 1.1 Got it! (3/3 → 1/3, merged heard 'Guyet, ask me anything about voice there.'); `en-glados` 1 Oh! (3/3 → 1/3, merged heard 'Oh ask me anything about voice there'); `en-glados` 1 Wow! (3/3 → 1/3, merged heard 'While ask me anything about voice there.'); `en-glados` 1.1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-glados` 1.1 Wow! (3/3 → 1/3, merged heard 'While ask me anything about voice there.'); `en-glados-high` 1 No! (2/3 → 1/3, merged heard 'Now ask me anything about voice there.'); `en-glados-high` 1.1 Great! (3/3 → 1/3, merged heard 'Great ask me anything about voice there.'); `en-glados-high` 1.1 Oh! (2/3 → 1/3, merged heard 'O past me anything of our voice there'); `en-glados-high` 1.1 Got it! (2/3 → 0/3, merged heard 'Kata ask me anything about voice there.'); `en-joe` 1 Wow! (2/3 → 0/3, merged heard 'Well, ask me anything about voice there.'); `en-joe` 1 Nope! (3/3 → 1/3, merged heard 'Nope, ask me anything about voice there.'); `en-joe` 1 Got it! (2/3 → 0/3, merged heard 'At it asked me anything about voice there.'); `en-joe` 1.1 Wow! (3/3 → 0/3, merged heard 'While ask me anything about voice there.'); `en-joe` 1.1 Nope! (2/3 → 0/3, merged heard 'No, ask me anything about voice there.'); `en-bryce` 1 Hi! (3/3 → 0/3, merged heard 'High ask me anything about voice there'); `en-bryce` 1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there'); `en-bryce` 1 Hello! (3/3 → 1/3, merged heard 'A lo ask me anything about voice there'); `en-bryce` 1 Oh! (3/3 → 1/3, merged heard 'Oh, ask me anything about voice there'); `en-bryce` 1.1 Hi! (3/3 → 0/3, merged heard 'High ask me anything about voice there'); `en-bryce` 1.1 Yes! (2/3 → 1/3, merged heard 'Guess ask me anything about voice there'); `en-bryce` 1.1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there'); `en-bryce` 1.1 Hello! (3/3 → 0/3, merged heard 'A lo ask me anything about voice there'); `en-bryce` 1.1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-john` 1 Hey! (2/3 → 1/3, merged heard 'Hey ask me anything about voice there'); `en-john` 1 Wow! (3/3 → 1/3, merged heard 'While ask me anything about voice there.'); `en-john` 1.1 Hi! (3/3 → 1/3, merged heard 'I ask me anything about voice there.'); `en-john` 1.1 Cool! (2/3 → 0/3, merged heard 'All ask me anything about voice there.'); `en-kristin` 1 Great! (3/3 → 1/3, merged heard 'Ray asked me anything about voice there.'); `en-kristin` 1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-kristin` 1 Wow! (2/3 → 1/3, merged heard 'While ask me anything above voice there.'); `en-kristin` 1 Cool! (2/3 → 1/3, merged heard 'Coul asked me anything about voice there.'); `en-kristin` 1.1 No! (3/3 → 1/3, merged heard 'Now ask me anything about voice there.'); `en-kristin` 1.1 Great! (2/3 → 1/3, merged heard 'Rate asked me anything about voice there.'); `en-kristin` 1.1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-kristin` 1.1 Wow! (3/3 → 1/3, merged heard 'While ask me anything about voice there'); `en-kristin` 1.1 Done! (2/3 → 1/3, merged heard 'Thun, ask me anything about voice there.'); `en-kathleen-low` 1 Hello! (2/3 → 0/3, merged heard 'Allo ask me anything about voice there'); `en-ljspeech` 1 Great! (2/3 → 1/3, merged heard 'Rate ask me anything about voice there'); `en-ljspeech-high` 1 Great! (3/3 → 0/3, merged heard 'Ray asked me anything about voice there'); `en-ljspeech-high` 1 Hello! (2/3 → 1/3, merged heard 'Hello asked me anything about voice there'); `en-ljspeech-high` 1 Wow! (3/3 → 0/3, merged heard 'Well, asked me anything about voice there.'); `en-ljspeech-high` 1.1 Yep! (2/3 → 1/3, merged heard 'Yeah, ask me anything about voice there.'); `en-arctic` 1 Hey! (2/3 → 1/3, merged heard 'He asked me anything about voice there'); `en-arctic` 1 Thanks! (2/3 → 1/3, merged heard 'Thinks ask me anything about voice there.'); `en-arctic` 1 Right! (2/3 → 0/3, merged heard 'Wright heart me anything about voice there'); `en-arctic` 1 Hello! (2/3 → 0/3, merged heard 'Although ask me anything about force there'); `en-arctic` 1 Oh! (3/3 → 1/3, merged heard 'Osk me anything about voice there'); `en-arctic` 1.1 Right! (2/3 → 0/3, merged heard 'Wright asked me anything about voice there.'); `en-arctic` 1.1 Hello! (2/3 → 1/3, merged heard 'Polo asked me anything about voice they are.'); `en-arctic` 1.1 Got it! (3/3 → 1/3, merged heard 'Cartech asked me anything about voice there.'); `en-hfc-female` 1 Wow! (2/3 → 1/3, merged heard 'While ask me anything about Boyce there'); `en-hfc-male` 1 Yep! (2/3 → 0/3, merged heard 'Yet ask me anything about voice there.'); `en-hfc-male` 1 Nope! (2/3 → 1/3, merged heard 'No, ask me anything about voice there'); `en-hfc-male` 1 Cool! (3/3 → 1/3, merged heard 'Who will ask me anything about voice there'); `en-hfc-male` 1.1 Nope! (2/3 → 1/3, merged heard 'No, ask me anything about voice there'); `en-hfc-male` 1.1 Got it! (2/3 → 0/3, merged heard 'Anna, ask me anything about voice there'); `en-gb-alan` 1 Wow! (2/3 → 0/3, merged heard 'Well, ask me anything about voice there'); `en-gb-alan` 1 Got it! (3/3 → 1/3, merged heard 'Got it ask me anything about voice there'); `en-gb-alan` 1.1 Wow! (3/3 → 0/3, merged heard 'Well, ask me anything about voice there'); `en-gb-alan` 1.1 Cool! (3/3 → 0/3, merged heard 'Call asked me anything about voice there'); `en-gb-alan` 1.1 Done! (2/3 → 0/3, merged heard 'Dun asked me anything about voice there.'); `en-gb-alan` 1.1 Got it! (3/3 → 0/3, merged heard 'ought it ask me anything about voice there'); `en-gb-cori` 1 Wow! (2/3 → 1/3, merged heard 'Wow, ask me anything about voice there.'); `en-gb-cori` 1.1 Nope! (2/3 → 0/3, merged heard 'No, ask me anything about voice there.'); `en-gb-jenny` 1 Right! (3/3 → 1/3, merged heard 'I ask me anything about voice there.'); `en-gb-jenny` 1 Nope! (2/3 → 1/3, merged heard 'No, ask me anything about voice there.'); `en-gb-jenny` 1 Done! (2/3 → 0/3, merged heard 'Jung ask me anything about voice there.'); `en-gb-jenny` 1.1 Right! (3/3 → 1/3, merged heard 'Right, ask me anything about voice there.'); `en-gb-jenny` 1.1 Wow! (3/3 → 1/3, merged heard 'Well, ask me anything about voice there.'); `en-gb-jenny` 1.1 Yep! (2/3 → 1/3, merged heard 'Yeah, ask me anything about voice there.'); `en-kitten-f2` 1.1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-kitten-m3` 1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there'); `en-kitten-m4` 1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there'); `en-kitten-m4` 1 Got it! (3/3 → 0/3, merged heard 'God it asked me anything about voice there'); `en-kitten-m4` 1.1 Got it! (3/3 → 0/3, merged heard 'God it asked me anything about voice there'); `en-kokoro-heart` 1 Got it! (3/3 → 0/3, merged heard 'God it asked me anything about voice there.'); `en-kokoro-bella` 1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there.'); `en-kokoro-nicole` 1.1 Hi! (3/3 → 0/3, merged heard 'I ask me anything about voice there.'); `en-kokoro-sarah` 1 Right! (3/3 → 0/3, merged heard 'Write, ask me anything about voice there.'); `en-kokoro-sarah` 1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-kokoro-sarah` 1 Wow! (3/3 → 0/3, merged heard 'Well, ask me anything about voice there.'); `en-kokoro-sarah` 1.1 Right! (3/3 → 0/3, merged heard 'Write, ask me anything about voice there.'); `en-kokoro-sarah` 1.1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-kokoro-michael` 1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there'); `en-kokoro-michael` 1 Oh! (3/3 → 1/3, merged heard 'O ask me anything about voice there'); `en-kokoro-michael` 1 Got it! (3/3 → 0/3, merged heard 'God it asked me anything about voice there.'); `en-kokoro-michael` 1.1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there.'); `en-kokoro-fenrir` 1 Wow! (3/3 → 0/3, merged heard 'Well, ask me anything about voice there.'); `en-kokoro-fenrir` 1 Got it! (3/3 → 0/3, merged heard 'God it asked me anything about voice there.'); `en-kokoro-fenrir` 1.1 Got it! (3/3 → 0/3, merged heard 'God it asked me anything about voice there.'); `en-kokoro-puck` 1 Got it! (3/3 → 0/3, merged heard 'God it ask me anything about voice there'); `en-gb-kokoro-emma` 1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there.'); `en-gb-kokoro-emma` 1.1 Right! (3/3 → 0/3, merged heard 'Write ask me anything about voice there.'); `en-gb-kokoro-emma` 1.1 Cool! (3/3 → 0/3, merged heard 'Coul asked me anything about voice there.'); `en-gb-kokoro-emma` 1.1 Got it! (3/3 → 0/3, merged heard 'Gart it asked me anything about voice there.'); `en-gb-kokoro-george` 1 Right! (2/3 → 0/3, merged heard 'Write ask me anything about voice there'); `en-gb-kokoro-george` 1 Oh! (3/3 → 0/3, merged heard 'O ask me anything about voice there'); `en-gb-kokoro-george` 1.1 Hi! (3/3 → 0/3, merged heard 'High ask me anything about voice there'); `en-gb-kokoro-george` 1.1 Right! (2/3 → 0/3, merged heard 'Write ask me anything about voice there').

- **By kind** (cells = model × speed × opener; `delta<-10` = median `delta_db` below −10 dB; STT = first word heard in the majority of 3):

  | kind         | form    | cells | delta_db < −10 | STT first-word hits | median delta_db | min delta_db |
  | ------------ | ------- | ----- | -------------- | ------------------- | --------------- | ------------ |
  | kitten       | written | 320   | 93             | 173                 | −6.4            | −24.1        |
  | kitten       | merged  | 320   | 0              | 303                 | 0.3             | −5.0         |
  | kokoro       | written | 360   | 0              | 328                 | 2.2             | −1.6         |
  | kokoro       | merged  | 360   | 0              | 321                 | 1.7             | −8.5         |
  | vits (Piper) | written | 1000  | 0              | 664                 | 0.9             | −8.9         |
  | vits (Piper) | merged  | 1000  | 0              | 825                 | 0.9             | −6.6         |

  The `delta_db < −10` quiet head appears only on Kitten. On Piper and Kokoro the "quiet opener" flag above comes from the STT
  first-word check alone. The check is a strict token match, so homophones count as misses ("Right" heard as "Write", "Hi" as
  "High", "Got it" as "God it", "Oh" as "O"); most "merged loses the first word" cells are of that shape.

## Decision

Measured on 42 English models (25 Piper, 8 Kitten, 9 Kokoro) × speeds {1.0, 1.1} × 20 openers × 2 forms × N = 3. Per cell: median of 3; an STT hit is the first word heard in at least 2 of 3.

- **Kitten, openers of at most 4 letters (240 cells):** written `delta_db < −10` in 75; merged never below −5 dB, quieter than written by more than 1 dB in 2 cells and by more than 3 dB in none; STT first word 115 → 228 hits; one cell lost (`en-kitten-f2` 1.1 "Oh!" heard as "O ask…", a homophone).
- **Kitten, 5 to 6 letters:** Hello! written median −10.7 dB (STT 22/48) → merged +1.7 (48/48); Great! −10.3 (40/48) → −0.1 (48/48); Thanks! −3.9 (29/48) → −0.2 (48/48); Right! −2.6 (31/48) → +0.4 (38/48, "Write" homophones).
- **Piper (VITS) and Kokoro:** no written cell below −10 dB anywhere; merged is quieter by more than 3 dB in 115 Piper and 22 Kokoro cells (worst Kokoro `en-kokoro-nicole` 1.1 "Yep!", −11.4 dB), and on Kokoro merging loses more first words than it gains for 5 to 6 letter and two-word openers.

Therefore the guard (`merge_short_opener` in `crates/speech/src/tts_text.rs`) applies to **Kitten models only**, to openers of **2 to 6 letters** (`MAX_OPENER_LETTERS`); two-word openers ("Got it!") never merge. Piper, Kokoro and cloud vendors keep their text. The merge changes the engine input and the phrase-cache key text only; captions, `agent_speak`, `assistant_*` events and history keep the original text.

The tables above were measured before the guard (`e8defdb3`).

## Method

`crates/vendor-sherpa-onnx/examples/opener_sweep.rs` synthesizes, for every English model of the staging Sherpa TTS catalog (42 English voices: 25 Piper, 8 Kitten nano speakers, 9 Kokoro speakers; 12 non-English voices skipped), each of 20 openers in two forms: as written (`"Hi! Ask me anything about VoiceThere."`) and merged (`"Hi, ask me anything about VoiceThere."`: the opener's `!` becomes `,`, the next word is lowercased). The text goes through the engine calls nwr makes (`normalize_phrase_text`, `engine_pieces` under `SentenceCase::Auto`, one `generate_with_config` per piece with the catalog speaker id), with the engine built by nwr's `create_offline_tts`. Speeds: 1 (nwr's Sherpa default: `parse_speed` in `src/tts.rs` returns 1.0 for every model kind when neither the TTS `model` field nor `SHERPA_TTS_SPEED` sets one) and 1.1. Clips are brought to 16 kHz mono with the product resampler (as `phrase_sweep`). Metrics in 10 ms frames: onset = first frame with RMS ≥ peak frame RMS − 40 dB, end = last such frame; `head_db` = RMS dBFS over [onset, onset+500 ms), `body_db` over [onset+500 ms, end]; `delta_db = head_db − body_db`; `gap_ms` = longest run below the threshold inside [onset, onset+1000 ms). STT: the kroko EN streaming recognizer (`sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06`) with 300 ms lead / 2500 ms tail silence; `first_word_ok` = first transcript token (lowercased, punctuation stripped) equals the opener's first word (`ok` ≡ `okay`). N = 3 syntheses per cell; tables show the median `delta_db` / `gap_ms`, and a cell counts as an STT hit when the first word is heard in the majority (2 of 3). Per-cell medians: [`tts-opener-sweep-2026-10-31.csv`](tts-opener-sweep-2026-10-31.csv) (one row per model × speed × opener × form, with the STT hits out of 3 and one sample transcript).

## Summary per model and speed

| model                 | kind   | speaker | speed | worst delta_db written | worst delta_db merged | min (merged − written) | first word written /20 | first word merged /20 |
| --------------------- | ------ | ------- | ----- | ---------------------- | --------------------- | ---------------------- | ---------------------- | --------------------- |
| `en`                  | vits   | 0       | 1     | 1.5                    | -0.4                  | -3.7                   | 16                     | 18                    |
| `en`                  | vits   | 0       | 1.1   | 1.3                    | -0.6                  | -3.4                   | 14                     | 18                    |
| `en-amy-low`          | vits   | 0       | 1     | -1.9                   | -3.3                  | -4.8                   | 7                      | 16                    |
| `en-amy-low`          | vits   | 0       | 1.1   | -2.6                   | -3.1                  | -4.5                   | 8                      | 17                    |
| `en-lessac`           | vits   | 0       | 1     | -2.3                   | -1.0                  | -3.6                   | 18                     | 18                    |
| `en-lessac`           | vits   | 0       | 1.1   | -0.9                   | -2.2                  | -5.2                   | 18                     | 18                    |
| `en-lessac-high`      | vits   | 0       | 1     | -1.3                   | -3.2                  | -6.2                   | 18                     | 19                    |
| `en-lessac-high`      | vits   | 0       | 1.1   | -2.4                   | -3.5                  | -5.8                   | 19                     | 19                    |
| `en-lessac-low`       | vits   | 0       | 1     | -1.4                   | -2.4                  | -4.5                   | 17                     | 20                    |
| `en-lessac-low`       | vits   | 0       | 1.1   | -1.1                   | -1.9                  | -4.6                   | 19                     | 20                    |
| `en-glados`           | vits   | 0       | 1     | -3.6                   | -2.2                  | -1.3                   | 18                     | 17                    |
| `en-glados`           | vits   | 0       | 1.1   | -3.8                   | -2.5                  | -0.7                   | 16                     | 16                    |
| `en-glados-high`      | vits   | 0       | 1     | -5.6                   | -3.0                  | -2.1                   | 13                     | 14                    |
| `en-glados-high`      | vits   | 0       | 1.1   | -4.2                   | -2.8                  | -2.7                   | 12                     | 13                    |
| `en-ryan`             | vits   | 0       | 1     | -1.2                   | -0.5                  | -0.6                   | 10                     | 20                    |
| `en-ryan`             | vits   | 0       | 1.1   | -1.1                   | -0.5                  | -0.3                   | 9                      | 20                    |
| `en-ryan-high`        | vits   | 0       | 1     | -0.9                   | -1.1                  | -2.6                   | 5                      | 19                    |
| `en-ryan-high`        | vits   | 0       | 1.1   | -0.7                   | -0.7                  | -1.8                   | 5                      | 19                    |
| `en-ryan-low`         | vits   | 0       | 1     | -2.3                   | -3.4                  | -4.3                   | 9                      | 18                    |
| `en-ryan-low`         | vits   | 0       | 1.1   | -2.9                   | -2.9                  | -4.2                   | 6                      | 20                    |
| `en-joe`              | vits   | 0       | 1     | 2.5                    | 3.7                   | -1.2                   | 16                     | 16                    |
| `en-joe`              | vits   | 0       | 1.1   | 2.2                    | 3.5                   | -0.4                   | 16                     | 16                    |
| `en-bryce`            | vits   | 0       | 1     | 1.1                    | 0.5                   | -3.4                   | 16                     | 15                    |
| `en-bryce`            | vits   | 0       | 1.1   | 0.3                    | 0.9                   | -3.2                   | 17                     | 14                    |
| `en-john`             | vits   | 0       | 1     | -5.1                   | -1.9                  | -1.1                   | 12                     | 13                    |
| `en-john`             | vits   | 0       | 1.1   | -6.2                   | -3.6                  | -0.1                   | 15                     | 15                    |
| `en-kristin`          | vits   | 0       | 1     | -4.4                   | -2.2                  | -5.0                   | 17                     | 15                    |
| `en-kristin`          | vits   | 0       | 1.1   | -4.7                   | -3.5                  | -4.7                   | 18                     | 14                    |
| `en-norman`           | vits   | 0       | 1     | -7.6                   | -3.0                  | -5.2                   | 6                      | 17                    |
| `en-norman`           | vits   | 0       | 1.1   | -8.2                   | -3.3                  | -3.4                   | 7                      | 17                    |
| `en-danny-low`        | vits   | 0       | 1     | -0.3                   | -6.6                  | -7.5                   | 8                      | 13                    |
| `en-danny-low`        | vits   | 0       | 1.1   | -0.6                   | -5.2                  | -6.7                   | 10                     | 14                    |
| `en-kathleen-low`     | vits   | 0       | 1     | -1.4                   | -2.5                  | -5.1                   | 5                      | 18                    |
| `en-kathleen-low`     | vits   | 0       | 1.1   | -2.8                   | -2.7                  | -5.0                   | 6                      | 20                    |
| `en-ljspeech`         | vits   | 0       | 1     | -2.9                   | -0.8                  | -2.8                   | 10                     | 13                    |
| `en-ljspeech`         | vits   | 0       | 1.1   | -2.8                   | -2.3                  | -2.6                   | 10                     | 14                    |
| `en-ljspeech-high`    | vits   | 0       | 1     | -4.8                   | -2.6                  | -5.7                   | 15                     | 15                    |
| `en-ljspeech-high`    | vits   | 0       | 1.1   | -5.1                   | -2.0                  | -1.8                   | 12                     | 16                    |
| `en-arctic`           | vits   | 0       | 1     | 1.7                    | -2.5                  | -6.1                   | 14                     | 10                    |
| `en-arctic`           | vits   | 0       | 1.1   | 2.3                    | -2.3                  | -5.2                   | 16                     | 15                    |
| `en-hfc-female`       | vits   | 0       | 1     | -2.1                   | -4.0                  | -4.6                   | 18                     | 19                    |
| `en-hfc-female`       | vits   | 0       | 1.1   | -2.8                   | -1.6                  | -2.4                   | 18                     | 20                    |
| `en-hfc-male`         | vits   | 0       | 1     | -0.8                   | -0.0                  | -2.2                   | 16                     | 15                    |
| `en-hfc-male`         | vits   | 0       | 1.1   | -0.8                   | -2.0                  | -2.0                   | 15                     | 16                    |
| `en-gb-alan`          | vits   | 0       | 1     | -1.8                   | -2.5                  | -5.1                   | 17                     | 17                    |
| `en-gb-alan`          | vits   | 0       | 1.1   | -2.2                   | -2.9                  | -5.5                   | 16                     | 15                    |
| `en-gb-cori`          | vits   | 0       | 1     | -8.9                   | -4.1                  | -1.4                   | 13                     | 17                    |
| `en-gb-cori`          | vits   | 0       | 1.1   | -7.9                   | -4.9                  | -2.3                   | 14                     | 16                    |
| `en-gb-jenny`         | vits   | 0       | 1     | -0.7                   | -2.2                  | -2.8                   | 17                     | 15                    |
| `en-gb-jenny`         | vits   | 0       | 1.1   | -1.9                   | -1.6                  | -1.9                   | 17                     | 16                    |
| `en-kitten-f2`        | kitten | 1       | 1     | -20.6                  | -2.1                  | 1.1                    | 15                     | 20                    |
| `en-kitten-f2`        | kitten | 1       | 1.1   | -21.9                  | -2.0                  | 0.4                    | 15                     | 19                    |
| `en-kitten-m2`        | kitten | 0       | 1     | -20.5                  | -5.0                  | -0.6                   | 7                      | 20                    |
| `en-kitten-m2`        | kitten | 0       | 1.1   | -22.5                  | -2.8                  | 0.9                    | 7                      | 18                    |
| `en-kitten-f3`        | kitten | 3       | 1     | -22.4                  | -1.2                  | 0.2                    | 14                     | 19                    |
| `en-kitten-f3`        | kitten | 3       | 1.1   | -15.1                  | -2.0                  | -0.0                   | 11                     | 20                    |
| `en-kitten-m3`        | kitten | 2       | 1     | -18.5                  | -3.4                  | -0.0                   | 11                     | 19                    |
| `en-kitten-m3`        | kitten | 2       | 1.1   | -18.5                  | -1.8                  | 2.2                    | 11                     | 19                    |
| `en-kitten-f4`        | kitten | 5       | 1     | -17.0                  | -2.9                  | 0.9                    | 14                     | 20                    |
| `en-kitten-f4`        | kitten | 5       | 1.1   | -24.1                  | -3.8                  | 0.9                    | 11                     | 20                    |
| `en-kitten-m4`        | kitten | 4       | 1     | -22.5                  | -3.9                  | 2.3                    | 7                      | 16                    |
| `en-kitten-m4`        | kitten | 4       | 1.1   | -23.6                  | -1.4                  | 2.8                    | 5                      | 15                    |
| `en-kitten-f5`        | kitten | 7       | 1     | -10.0                  | -2.7                  | 0.4                    | 10                     | 19                    |
| `en-kitten-f5`        | kitten | 7       | 1.1   | -13.4                  | -0.5                  | 2.6                    | 8                      | 19                    |
| `en-kitten-m5`        | kitten | 6       | 1     | -12.4                  | -3.0                  | -1.9                   | 14                     | 20                    |
| `en-kitten-m5`        | kitten | 6       | 1.1   | -12.6                  | -1.5                  | -1.4                   | 13                     | 20                    |
| `en-kokoro-heart`     | kokoro | 3       | 1     | 1.1                    | -0.5                  | -2.6                   | 19                     | 19                    |
| `en-kokoro-heart`     | kokoro | 3       | 1.1   | 0.9                    | -0.6                  | -2.6                   | 20                     | 20                    |
| `en-kokoro-bella`     | kokoro | 2       | 1     | -1.6                   | -1.4                  | -2.4                   | 20                     | 19                    |
| `en-kokoro-bella`     | kokoro | 2       | 1.1   | -1.6                   | -1.4                  | -2.0                   | 20                     | 20                    |
| `en-kokoro-nicole`    | kokoro | 6       | 1     | 2.9                    | -4.3                  | -7.6                   | 16                     | 18                    |
| `en-kokoro-nicole`    | kokoro | 6       | 1.1   | 2.2                    | -8.5                  | -11.4                  | 18                     | 18                    |
| `en-kokoro-sarah`     | kokoro | 9       | 1     | 2.0                    | -0.8                  | -3.1                   | 20                     | 17                    |
| `en-kokoro-sarah`     | kokoro | 9       | 1.1   | 1.5                    | -0.5                  | -2.5                   | 17                     | 17                    |
| `en-kokoro-michael`   | kokoro | 16      | 1     | 2.4                    | 0.8                   | -3.4                   | 19                     | 17                    |
| `en-kokoro-michael`   | kokoro | 16      | 1.1   | 2.0                    | 0.5                   | -2.2                   | 19                     | 19                    |
| `en-kokoro-fenrir`    | kokoro | 14      | 1     | 0.3                    | -0.6                  | -2.3                   | 19                     | 18                    |
| `en-kokoro-fenrir`    | kokoro | 14      | 1.1   | -0.3                   | -1.5                  | -2.4                   | 19                     | 18                    |
| `en-kokoro-puck`      | kokoro | 18      | 1     | 1.3                    | 1.3                   | -0.9                   | 18                     | 18                    |
| `en-kokoro-puck`      | kokoro | 18      | 1.1   | 0.2                    | 0.9                   | -0.6                   | 16                     | 17                    |
| `en-gb-kokoro-emma`   | kokoro | 21      | 1     | 0.8                    | 1.3                   | -0.6                   | 18                     | 18                    |
| `en-gb-kokoro-emma`   | kokoro | 21      | 1.1   | -0.1                   | 1.2                   | -0.2                   | 19                     | 16                    |
| `en-gb-kokoro-george` | kokoro | 26      | 1     | 2.2                    | 0.1                   | -2.6                   | 16                     | 16                    |
| `en-gb-kokoro-george` | kokoro | 26      | 1.1   | 1.5                    | 0.1                   | -2.6                   | 15                     | 16                    |

## Per-opener detail

Every Kitten speaker, plus every other model × speed where some opener as written has median `delta_db < -10` or STT misses the first word. `hits` = syntheses (of 3) whose first transcript word is the opener's.

### `en` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 2.8              | 1.6             | -1.2             | 70             | 150           | 3/3          | 3/3         | Hi ask me anything about voice there       |
| Hey!    | 2.0              | 0.0             | -2.0             | 60             | 150           | 3/3          | 3/3         | Hey, ask me anything about voice there     |
| Sure!   | 3.5              | 0.7             | -2.8             | 60             | 120           | 3/3          | 3/3         | Sure, ask me anything about voice there    |
| Yes!    | 2.2              | 1.4             | -0.8             | 40             | 30            | 3/3          | 3/3         | Yes ask me anything about voice there      |
| No!     | 4.7              | 2.4             | -2.3             | 190            | 50            | 3/3          | 3/3         | No, ask me anything about voice there      |
| Okay!   | 2.1              | -0.4            | -2.5             | 50             | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 3.3              | 2.0             | -1.3             | 210            | 170           | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 1.5              | 0.3             | -1.2             | 120            | 60            | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | 1.8              | 0.4             | -1.4             | 150            | 150           | 3/3          | 3/3         | Thanks. Ask me anything about voice there  |
| Right!  | 2.4              | 2.2             | -0.2             | 150            | 50            | 3/3          | 3/3         | Right, ask me anything about voice there   |
| Hello!  | 6.0              | 5.0             | -1.0             | 190            | 160           | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 3.3              | 1.1             | -2.2             | 190            | 140           | 3/3          | 3/3         | Well, ask me anything about voice there    |
| So!     | 4.3              | 0.6             | -3.7             | 200            | 40            | 3/3          | 3/3         | So, ask me anything about voice there      |
| Oh!     | 3.3              | 0.8             | -2.5             | 150            | 170           | 3/3          | 3/3         | Oh ask me anything about voice there       |
| Wow!    | 2.9              | 1.6             | -1.3             | 190            | 170           | 3/3          | 3/3         | Wow, ask me anything about voice there     |
| Yep!    | 2.3              | 1.2             | -1.1             | 140            | 120           | 0/3          | 3/3         | Ep ask me anything about voice there       |
| Nope!   | 2.6              | 1.6             | -1.0             | 120            | 100           | 3/3          | 3/3         | Nope, ask me anything about voice there    |
| Cool!   | 3.3              | 1.9             | -1.5             | 170            | 120           | 1/3          | 0/3         | Cool ask me anything about voice there     |
| Done!   | 3.2              | 2.0             | -1.2             | 200            | 160           | 0/3          | 3/3         | Then, ask me anything about voice there.   |
| Got it! | 3.8              | 2.6             | -1.2             | 70             | 60            | 1/3          | 0/3         | Cart it? Ask me anything about voice there |

### `en` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)              |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | -------------------------------------------- |
| Hi!     | 3.1              | 1.0             | -2.1             | 60             | 150           | 3/3          | 3/3         | Hi ask me anything about voice there         |
| Hey!    | 1.8              | -0.6            | -2.4             | 190            | 140           | 3/3          | 3/3         | Hey, ask me anything about voice there       |
| Sure!   | 2.9              | 0.4             | -2.5             | 200            | 110           | 3/3          | 3/3         | Sure, ask me anything about voice there      |
| Yes!    | 2.3              | 0.1             | -2.1             | 180            | 150           | 1/3          | 3/3         | Guess, ask me anything about voice there     |
| No!     | 4.1              | 1.9             | -2.2             | 170            | 150           | 3/3          | 3/3         | No, ask me anything about voice there        |
| Okay!   | 2.3              | 0.2             | -2.1             | 200            | 30            | 1/3          | 3/3         | The K ask me anything about voice there      |
| OK!     | 3.4              | 1.0             | -2.4             | 200            | 150           | 3/3          | 3/3         | Okay, ask me anything about voice there      |
| Great!  | 1.3              | 0.1             | -1.2             | 110            | 140           | 3/3          | 3/3         | Great. Ask me anything about voice there     |
| Thanks! | 2.2              | 0.4             | -1.8             | 150            | 150           | 3/3          | 3/3         | Thanks. Ask me anything about voice there    |
| Right!  | 2.4              | 0.1             | -2.3             | 150            | 140           | 3/3          | 3/3         | Right, ask me anything about voice there     |
| Hello!  | 6.5              | 4.5             | -2.0             | 180            | 150           | 3/3          | 3/3         | Hello ask me anything about voice there      |
| Well!   | 3.5              | 2.5             | -1.0             | 160            | 110           | 3/3          | 3/3         | Well, ask me anything about voice there      |
| So!     | 4.0              | 0.6             | -3.4             | 170            | 160           | 3/3          | 3/3         | So ask me anything about voice there         |
| Oh!     | 2.5              | 2.1             | -0.4             | 140            | 140           | 3/3          | 3/3         | Oh, ask me anything about voice there        |
| Wow!    | 2.8              | 1.5             | -1.2             | 160            | 140           | 3/3          | 3/3         | Wow, ask me anything about voice there       |
| Yep!    | 1.8              | 1.0             | -0.9             | 130            | 120           | 0/3          | 3/3         | Dip ask me anything about voice there        |
| Nope!   | 2.8              | 1.4             | -1.4             | 90             | 120           | 3/3          | 3/3         | Nope, ask me anything about voice there      |
| Cool!   | 3.0              | 1.3             | -1.6             | 140            | 120           | 0/3          | 0/3         | Will ask me anything about voice there       |
| Done!   | 2.7              | 1.4             | -1.3             | 180            | 120           | 0/3          | 3/3         | Then ask me anything about voice there       |
| Got it! | 3.9              | 2.3             | -1.6             | 200            | 50            | 0/3          | 0/3         | Caught it, ask me anything about voice there |

### `en-amy-low` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)               |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | --------------------------------------------- |
| Hi!     | 1.3              | 1.4             | 0.1              | 180            | 50            | 1/3          | 0/3         | Hide ask me anything about voice there        |
| Hey!    | 0.7              | 1.2             | 0.5              | 160            | 60            | 1/3          | 3/3         | They ask me anything about voice there        |
| Sure!   | 0.0              | 1.7             | 1.7              | 150            | 50            | 0/3          | 3/3         | Joe asked me anything about voice there       |
| Yes!    | -1.9             | -2.7            | -0.8             | 150            | 40            | 1/3          | 0/3         | Yeah, ask me anything about voice there       |
| No!     | 0.3              | 1.1             | 0.8              | 40             | 50            | 0/3          | 3/3         | Now ask me anything about voice there         |
| Okay!   | 0.3              | 0.6             | 0.3              | 160            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there       |
| OK!     | 1.5              | 2.1             | 0.6              | 160            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there       |
| Great!  | -0.3             | -0.7            | -0.4             | 100            | 110           | 1/3          | 3/3         | Bright, ask me anything about voice there     |
| Thanks! | 1.5              | -3.3            | -4.8             | 40             | 40            | 3/3          | 3/3         | Thanks, ask me anything about voice there     |
| Right!  | 1.4              | -0.9            | -2.2             | 120            | 130           | 2/3          | 3/3         | Right, ask me anything about voice there      |
| Hello!  | 2.8              | 2.8             | 0.0              | 120            | 80            | 2/3          | 3/3         | Hellout, ask me anything about voice there    |
| Well!   | 2.4              | 4.2             | 1.8              | 130            | 20            | 3/3          | 3/3         | Well, ask me anything about voice there       |
| So!     | 0.1              | 1.4             | 1.3              | 160            | 60            | 0/3          | 3/3         | Died, ask me anything about voice there       |
| Oh!     | -1.6             | 1.7             | 3.3              | 130            | 60            | 0/3          | 2/3         | An ask me anything about voice there          |
| Wow!    | 2.3              | 2.9             | 0.6              | 160            | 40            | 0/3          | 3/3         | Lyad asked me anything about voice there      |
| Yep!    | -1.2             | 0.7             | 1.9              | 140            | 70            | 0/3          | 0/3         | An, ask me anything about voice there         |
| Nope!   | -0.1             | -0.1            | 0.1              | 110            | 60            | 2/3          | 3/3         | Nope ask me anything about voice there        |
| Cool!   | 2.0              | 1.3             | -0.7             | 150            | 70            | 0/3          | 2/3         | Fool, ask me anything about voice there       |
| Done!   | -0.1             | 1.7             | 1.8              | 100            | 40            | 1/3          | 1/3         | Done ask me anything about voice there        |
| Got it! | 3.3              | -0.4            | -3.6             | 160            | 140           | 0/3          | 3/3         | thought it ask me anything about voice there. |

### `en-amy-low` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 0.4              | 1.9             | 1.5              | 140            | 60            | 0/3          | 2/3         | I ask me anything about voice there.       |
| Hey!    | 0.2              | 0.6             | 0.5              | 150            | 50            | 0/3          | 3/3         | They asked me anything about voice there   |
| Sure!   | -1.1             | 1.2             | 2.2              | 140            | 50            | 0/3          | 3/3         | Cheer asked me anything about voice there. |
| Yes!    | -2.6             | -0.7            | 1.9              | 130            | 40            | 0/3          | 0/3         | Yet ask me anything about voice there      |
| No!     | -0.5             | 1.6             | 2.1              | 160            | 50            | 1/3          | 3/3         | Now ask me anything about voice there      |
| Okay!   | 0.5              | -0.4            | -1.0             | 160            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 0.9              | 1.6             | 0.7              | 160            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | -0.4             | -0.6            | -0.2             | 90             | 100           | 2/3          | 3/3         | Bright, ask me anything about voice there  |
| Thanks! | 1.4              | -3.1            | -4.5             | 160            | 40            | 3/3          | 3/3         | Thanks, ask me anything about voice there  |
| Right!  | 1.0              | 1.1             | 0.1              | 120            | 70            | 2/3          | 3/3         | Right, ask me anything about voice there   |
| Hello!  | 2.5              | 2.7             | 0.1              | 110            | 50            | 2/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 2.9              | 3.4             | 0.6              | 130            | 20            | 3/3          | 3/3         | Well, ask me anything about voice there    |
| So!     | 0.6              | 0.9             | 0.3              | 130            | 50            | 0/3          | 3/3         | Tig, ask me anything about voice there     |
| Oh!     | -2.0             | 2.5             | 4.5              | 120            | 30            | 0/3          | 3/3         | An ask me anything about voice there       |
| Wow!    | 1.8              | 3.0             | 1.3              | 100            | 40            | 0/3          | 2/3         | Live asked me anything about voice there.  |
| Yep!    | -1.0             | 1.6             | 2.6              | 110            | 80            | 0/3          | 0/3         | Dad asked me anything about voice there    |
| Nope!   | -0.7             | -0.9            | -0.3             | 90             | 50            | 2/3          | 3/3         | Nope ask me anything about voice there     |
| Cool!   | 1.4              | 1.0             | -0.3             | 120            | 80            | 0/3          | 2/3         | Pool asked me anything about voice there   |
| Done!   | -0.9             | 1.9             | 2.8              | 100            | 30            | 1/3          | 1/3         | Then ask me anything about voice there     |
| Got it! | 2.6              | -0.4            | -3.0             | 130            | 150           | 1/3          | 3/3         | got it ask me anything about voice there   |

### `en-lessac` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 1.2              | 1.4             | 0.2              | 230            | 60            | 2/3          | 3/3         | Pi. Ask me anything about voice there.     |
| Hey!    | 2.1              | -0.1            | -2.2             | 230            | 80            | 3/3          | 3/3         | Hey, ask me anything about voice there     |
| Sure!   | 1.1              | -0.3            | -1.4             | 240            | 70            | 2/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | -0.0             | 2.6             | 2.6              | 190            | 60            | 2/3          | 3/3         | Yes, ask me anything about voice there     |
| No!     | 1.2              | 4.2             | 3.0              | 220            | 160           | 2/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.4              | -0.2            | -2.6             | 100            | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 4.7              | 2.5             | -2.3             | 230            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 2.3              | -0.3            | -2.6             | 170            | 90            | 3/3          | 3/3         | Great. Ask me anything about voice there.  |
| Thanks! | 1.5              | -1.0            | -2.5             | 140            | 110           | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | -0.5             | 3.3             | 3.8              | 200            | 80            | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 3.1              | 3.0             | -0.1             | 210            | 80            | 3/3          | 3/3         | Hello Ask me anything about voice there    |
| Well!   | 1.4              | 1.8             | 0.4              | 90             | 150           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 1.4              | 2.7             | 1.3              | 220            | 140           | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 1.4              | 5.4             | 4.0              | 180            | 100           | 2/3          | 2/3         | Bo, ask me anything about voice there.     |
| Wow!    | 0.2              | 3.7             | 3.5              | 220            | 50            | 3/3          | 2/3         | Wow, ask me anything about voice there.    |
| Yep!    | -0.6             | 2.8             | 3.4              | 130            | 80            | 3/3          | 2/3         | Yep, ask me anything about voice there.    |
| Nope!   | 2.1              | 2.9             | 0.8              | 210            | 80            | 0/3          | 2/3         | Bope, ask me anything about voice there.   |
| Cool!   | 3.8              | 0.2             | -3.6             | 180            | 90            | 1/3          | 2/3         | Cool, ask me anything about voice there.   |
| Done!   | 1.6              | 2.5             | 1.0              | 160            | 130           | 3/3          | 1/3         | Done. Ask me anything about voice there.   |
| Got it! | -2.3             | -0.7            | 1.6              | 170            | 60            | 2/3          | 1/3         | Got it? Ask me anything about voice there. |

### `en-lessac` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)             |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------- |
| Hi!     | 0.5              | 2.4             | 2.0              | 180            | 60            | 3/3          | 3/3         | Hi. Ask me anything about voice there       |
| Hey!    | 0.9              | 1.9             | 1.0              | 220            | 40            | 3/3          | 3/3         | Hey, ask me anything about toys there.      |
| Sure!   | 2.1              | 1.1             | -1.0             | 330            | 50            | 2/3          | 3/3         | Sure, ask me anything about voice there.    |
| Yes!    | 0.1              | 3.0             | 2.8              | 230            | 40            | 3/3          | 3/3         | Yes, ask me anything about voice there.     |
| No!     | 1.4              | 2.0             | 0.6              | 350            | 140           | 2/3          | 3/3         | No, ask me anything about voice there.      |
| Okay!   | 0.7              | -0.5            | -1.2             | 90             | 140           | 3/3          | 3/3         | Okay, ask me anything about voice there.    |
| OK!     | 4.0              | 1.9             | -2.1             | 130            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there.    |
| Great!  | 1.6              | 2.6             | 1.0              | 160            | 50            | 3/3          | 3/3         | Great. Ask me anything about voice there.   |
| Thanks! | 2.2              | -1.4            | -3.6             | 120            | 40            | 3/3          | 3/3         | Thanks. Ask me anything about voice there.  |
| Right!  | -0.3             | 3.0             | 3.3              | 180            | 120           | 3/3          | 3/3         | Right, ask me anything about voice there.   |
| Hello!  | 3.6              | 3.9             | 0.3              | 220            | 70            | 3/3          | 3/3         | Hello ask me anything about voice there     |
| Well!   | 0.8              | 3.4             | 2.6              | 240            | 90            | 3/3          | 3/3         | Well, ask me anything about voice there.    |
| So!     | 0.7              | 3.5             | 2.8              | 210            | 100           | 3/3          | 3/3         | So, ask me anything about voice there.      |
| Oh!     | 0.7              | 4.3             | 3.7              | 230            | 50            | 3/3          | 2/3         | Oh, ask me anything about voice there.      |
| Wow!    | 0.8              | 3.2             | 2.4              | 230            | 30            | 3/3          | 0/3         | Wow, ask me anything about voice there.     |
| Yep!    | -0.8             | 3.5             | 4.3              | 220            | 140           | 0/3          | 3/3         | Hip, ask me anything about voice there.     |
| Nope!   | 2.5              | 2.0             | -0.6             | 130            | 160           | 0/3          | 2/3         | Op ask me anything about voice there.       |
| Cool!   | 3.0              | -2.2            | -5.2             | 190            | 90            | 3/3          | 3/3         | Cool, ask me anything about voice there.    |
| Done!   | 0.8              | 2.9             | 2.1              | 210            | 80            | 3/3          | 2/3         | Done. Ask me anything about voice there.    |
| Got it! | -0.9             | 1.2             | 2.1              | 190            | 50            | 2/3          | 0/3         | That it? Ask me anything about voice there. |

### `en-lessac-high` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 1.6              | -0.8            | -2.4             | 180            | 120           | 2/3          | 3/3         | I ask me anything about voice there.       |
| Hey!    | 2.1              | -1.6            | -3.6             | 40             | 110           | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 2.0              | -1.3            | -3.4             | 150            | 100           | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | -1.3             | -2.2            | -0.8             | 160            | 110           | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 2.9              | 0.4             | -2.4             | 60             | 40            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 3.9              | -2.3            | -6.2             | 150            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 4.0              | 0.1             | -3.9             | 180            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 0.7              | 0.0             | -0.7             | 140            | 70            | 3/3          | 3/3         | Great. Ask me anything about voice there.  |
| Thanks! | 1.4              | -3.2            | -4.7             | 140            | 50            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 0.7              | -0.7            | -1.4             | 100            | 120           | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 3.5              | -0.2            | -3.7             | 160            | 40            | 3/3          | 3/3         | Hello ask me anything about voice there.   |
| Well!   | 3.0              | -2.1            | -5.1             | 150            | 80            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 3.5              | -1.6            | -5.1             | 110            | 50            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 1.3              | -0.5            | -1.7             | 90             | 40            | 3/3          | 3/3         | Oh, ask me anything about voice there      |
| Wow!    | 0.6              | 1.4             | 0.8              | 100            | 40            | 3/3          | 3/3         | Wow, ask me anything about voice there.    |
| Yep!    | 0.5              | -1.3            | -1.8             | 130            | 60            | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 3.4              | -0.0            | -3.4             | 80             | 50            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 4.6              | -0.5            | -5.1             | 180            | 50            | 3/3          | 1/3         | Cool, ask me anything about voice there.   |
| Done!   | 0.8              | 3.0             | 2.1              | 90             | 40            | 1/3          | 3/3         | Un, ask me anything about voice there.     |
| Got it! | 2.6              | 2.5             | -0.2             | 60             | 50            | 0/3          | 3/3         | Audit ask me anything about voice there.   |

### `en-lessac-high` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 0.0              | -2.5            | -2.5             | 130            | 110           | 2/3          | 3/3         | FI ask me anything about voice there.      |
| Hey!    | 2.0              | 0.2             | -1.8             | 30             | 50            | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 1.4              | -0.4            | -1.8             | 110            | 110           | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | -2.4             | -2.4            | 0.0              | 100            | 80            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 3.1              | -1.7            | -4.8             | 110            | 120           | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 3.0              | -2.7            | -5.8             | 140            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 2.5              | -0.3            | -2.8             | 150            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | -0.2             | 0.4             | 0.6              | 100            | 50            | 3/3          | 3/3         | Great Ask me anything about voice there.   |
| Thanks! | 1.6              | -3.5            | -5.1             | 100            | 80            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 0.1              | -1.9            | -2.1             | 70             | 90            | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 3.6              | -0.2            | -3.9             | 90             | 40            | 2/3          | 3/3         | Hello ask me anything about voice there.   |
| Well!   | 3.1              | -1.0            | -4.1             | 110            | 100           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 2.4              | 0.4             | -2.0             | 60             | 40            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 0.1              | -1.4            | -1.5             | 80             | 130           | 3/3          | 2/3         | Oh, ask me anything about voice there.     |
| Wow!    | -0.2             | 1.9             | 2.1              | 100            | 30            | 2/3          | 3/3         | Wow, ask me anything about voice there.    |
| Yep!    | 0.4              | -0.3            | -0.7             | 100            | 50            | 2/3          | 3/3         | Gepp ask me anything about voice there.    |
| Nope!   | 2.6              | -0.3            | -2.9             | 70             | 50            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 3.2              | -0.7            | -3.9             | 100            | 70            | 2/3          | 3/3         | Whole ask me anything about voice there.   |
| Done!   | 0.6              | 1.2             | 0.6              | 160            | 40            | 0/3          | 3/3         | Bun, ask me anything about voice there.    |
| Got it! | 1.3              | 2.4             | 1.0              | 170            | 40            | 3/3          | 1/3         | Got it. Ask me anything about voice there. |

### `en-lessac-low` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -0.2             | 0.9             | 1.2              | 150            | 50            | 3/3          | 3/3         | Hi, ask me anything about voice there.     |
| Hey!    | 1.8              | -1.2            | -3.0             | 180            | 80            | 2/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | -0.5             | -1.3            | -0.8             | 80             | 70            | 2/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 2.3              | -2.3            | -4.5             | 130            | 130           | 0/3          | 3/3         | Desk me anything about voice there.        |
| No!     | 2.1              | 2.2             | 0.1              | 180            | 50            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 3.4              | -0.9            | -4.3             | 80             | 50            | 2/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 5.0              | 0.9             | -4.1             | 170            | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 3.4              | 2.3             | -1.1             | 110            | 50            | 3/3          | 3/3         | great. Ask me anything about voice there.  |
| Thanks! | 2.0              | -2.4            | -4.4             | 150            | 50            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 0.0              | 2.9             | 2.8              | 110            | 50            | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 1.9              | 0.8             | -1.1             | 180            | 40            | 3/3          | 3/3         | Hello Ask me anything about voice there.   |
| Well!   | 2.1              | 0.1             | -2.1             | 170            | 110           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 2.1              | 1.2             | -0.9             | 170            | 40            | 3/3          | 3/3         | So, ask me anything about voice there.     |
| Oh!     | 1.6              | 0.2             | -1.3             | 160            | 70            | 3/3          | 3/3         | Oh, ask me anything about voice there.     |
| Wow!    | -0.3             | 1.8             | 2.0              | 180            | 50            | 2/3          | 3/3         | Well, ask me anything about voice there.   |
| Yep!    | 1.3              | 0.2             | -1.0             | 150            | 110           | 2/3          | 2/3         | Yep, ask me anything about voice there.    |
| Nope!   | 3.2              | 2.4             | -0.8             | 90             | 60            | 3/3          | 2/3         | Nope, ask me anything about voice there.   |
| Cool!   | 3.2              | 0.2             | -3.0             | 240            | 110           | 1/3          | 3/3         | Fool, ask me anything about voice there.   |
| Done!   | -1.4             | 1.5             | 2.8              | 100            | 120           | 3/3          | 3/3         | Done. Ask me anything about voice there.   |
| Got it! | 0.8              | 1.1             | 0.3              | 50             | 40            | 0/3          | 3/3         | On it ask me anything about voice there.   |

### `en-lessac-low` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -0.0             | -0.7            | -0.7             | 100            | 80            | 3/3          | 3/3         | Hi ask me anything about voice there.      |
| Hey!    | 2.2              | -1.1            | -3.3             | 190            | 140           | 3/3          | 3/3         | Hey ask me anything about voice there.     |
| Sure!   | 0.5              | 1.3             | 0.8              | 190            | 70            | 3/3          | 2/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.9              | -1.9            | -3.8             | 110            | 110           | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 1.4              | 1.9             | 0.5              | 220            | 40            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 3.2              | -0.7            | -3.9             | 180            | 120           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 5.0              | 0.5             | -4.6             | 190            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 2.9              | 1.9             | -1.0             | 200            | 40            | 3/3          | 3/3         | Great ask me anything about voice there.   |
| Thanks! | 2.5              | -0.7            | -3.1             | 110            | 150           | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 2.0              | 2.7             | 0.7              | 150            | 40            | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 1.8              | 0.8             | -0.9             | 160            | 50            | 3/3          | 3/3         | Hello Ask me anything about voice there    |
| Well!   | 0.3              | -0.5            | -0.7             | 170            | 110           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 2.0              | -1.4            | -3.4             | 170            | 160           | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 2.0              | 0.2             | -1.8             | 160            | 130           | 3/3          | 3/3         | Oh, ask me anything about voice there.     |
| Wow!    | 0.5              | 2.5             | 1.9              | 150            | 30            | 2/3          | 3/3         | Well, ask me anything about voice there.   |
| Yep!    | 2.1              | 0.6             | -1.5             | 130            | 60            | 2/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 3.0              | 1.9             | -1.0             | 90             | 140           | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 3.4              | 1.1             | -2.3             | 150            | 40            | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | -1.1             | 1.7             | 2.7              | 110            | 140           | 3/3          | 3/3         | Done. Ask me anything about voice there.   |
| Got it! | -0.0             | 1.4             | 1.4              | 190            | 30            | 0/3          | 3/3         | Ant ask me anything about voice there.     |

### `en-glados` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -1.4             | 0.4             | 1.8              | 160            | 160           | 2/3          | 3/3         | Hi ask me anything about voice there.      |
| Hey!    | -2.1             | -1.0            | 1.1              | 150            | 160           | 2/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | -1.0             | -0.7            | 0.3              | 130            | 130           | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | -1.0             | -0.8            | 0.2              | 100            | 100           | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | -0.7             | 0.6             | 1.4              | 160            | 40            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | -0.0             | -0.3            | -0.3             | 160            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 1.0              | 1.1             | 0.1              | 170            | 150           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 0.0              | -1.2            | -1.3             | 140            | 60            | 3/3          | 2/3         | Great. Ask me anything about voice there.  |
| Thanks! | 0.0              | -0.4            | -0.4             | 170            | 100           | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | -3.6             | -2.2            | 1.4              | 120            | 60            | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 1.8              | 2.2             | 0.4              | 160            | 40            | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 0.0              | -0.9            | -0.9             | 100            | 110           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | -1.1             | 0.1             | 1.2              | 100            | 20            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | -1.3             | 0.1             | 1.4              | 110            | 30            | 3/3          | 1/3         | Oh, ask me anything about voice there.     |
| Wow!    | -1.6             | -0.0            | 1.6              | 130            | 20            | 3/3          | 1/3         | Wow, ask me anything about voice there.    |
| Yep!    | -2.4             | -1.6            | 0.8              | 120            | 140           | 1/3          | 2/3         | Yep, ask me anything about voice there.    |
| Nope!   | -0.2             | 0.2             | 0.4              | 100            | 50            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 0.8              | 0.5             | -0.3             | 130            | 40            | 1/3          | 0/3         | Cohole. Ask me anything about voice there. |
| Done!   | -2.2             | -0.3            | 2.0              | 100            | 160           | 3/3          | 3/3         | Done ask me anything about voice there.    |
| Got it! | -2.5             | -1.6            | 0.9              | 140            | 180           | 2/3          | 3/3         | Got it? Ask me anything about voice there. |

### `en-glados` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -2.1             | -0.4            | 1.7              | 150            | 90            | 1/3          | 3/3         | I ask me anything about voice there.       |
| Hey!    | -2.4             | -0.8            | 1.6              | 120            | 150           | 1/3          | 3/3         | Okay, ask me anything about voice there.   |
| Sure!   | -1.6             | -1.1            | 0.6              | 160            | 100           | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | -1.7             | -0.8            | 0.9              | 70             | 180           | 2/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | -1.4             | 1.1             | 2.5              | 190            | 30            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | -0.8             | -0.7            | 0.1              | 230            | 160           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 0.9              | 1.3             | 0.4              | 170            | 120           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | -0.8             | -1.2            | -0.4             | 110            | 50            | 3/3          | 2/3         | Great. Ask me anything about voice there.  |
| Thanks! | -0.8             | -0.7            | 0.1              | 60             | 60            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | -3.8             | -2.5            | 1.3              | 140            | 80            | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 1.3              | 1.6             | 0.3              | 140            | 40            | 3/3          | 3/3         | Hello, ask me anything about voice there.  |
| Well!   | -0.1             | -0.8            | -0.7             | 110            | 100           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | -2.1             | -0.3            | 1.8              | 80             | 20            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | -2.4             | -0.4            | 1.9              | 140            | 20            | 3/3          | 0/3         | Oh, ask me anything about voice there.     |
| Wow!    | -1.8             | -0.6            | 1.2              | 100            | 30            | 3/3          | 1/3         | Wow, ask me anything about voice there.    |
| Yep!    | -2.6             | -1.4            | 1.1              | 130            | 140           | 0/3          | 1/3         | Depp ask me anything about voice there.    |
| Nope!   | -1.0             | 0.5             | 1.5              | 80             | 170           | 3/3          | 3/3         | Nope ask me anything about voice there.    |
| Cool!   | 0.6              | 0.3             | -0.3             | 120            | 160           | 1/3          | 0/3         | Cool, ask me anything about voice there.   |
| Done!   | -2.1             | -1.1            | 1.0              | 80             | 160           | 3/3          | 3/3         | Done, ask me anything about voice there.   |
| Got it! | -3.1             | -1.2            | 1.9              | 130            | 210           | 3/3          | 2/3         | Got it, ask me anything about voice there. |

### `en-glados-high` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -1.2             | -1.9            | -0.7             | 50             | 0             | 0/3          | 0/3         | Pi ask me anything about voice there.      |
| Hey!    | -1.0             | -0.7            | 0.3              | 0              | 0             | 2/3          | 2/3         | Hey, ask me anything about voice there.    |
| Sure!   | -0.9             | 0.7             | 1.6              | 70             | 0             | 3/3          | 2/3         | Sure, ask me anything about voice there.   |
| Yes!    | -0.0             | -1.4            | -1.3             | 30             | 40            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | -1.3             | -0.1            | 1.2              | 40             | 70            | 2/3          | 1/3         | No, ask me anything about voice there.     |
| Okay!   | -0.4             | -0.1            | 0.3              | 30             | 0             | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | -0.2             | 0.4             | 0.6              | 20             | 0             | 3/3          | 2/3         | Okay, ask me anything about voice there.   |
| Great!  | -5.6             | -1.3            | 4.3              | 80             | 50            | 3/3          | 2/3         | Great. Ask me anything about voice there.  |
| Thanks! | -1.2             | -1.4            | -0.2             | 20             | 60            | 3/3          | 3/3         | Thanks ask me anything about voice there.  |
| Right!  | -4.0             | -3.0            | 1.0              | 70             | 60            | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | -3.4             | 0.4             | 3.8              | 10             | 20            | 1/3          | 3/3         | Hello ask me anything about Vistare.       |
| Well!   | -3.3             | -1.5            | 1.8              | 100            | 50            | 2/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 0.4              | -1.7            | -2.1             | 60             | 0             | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | -2.0             | -0.8            | 1.2              | 10             | 60            | 0/3          | 0/3         | Ask me anything about voice there.         |
| Wow!    | -1.6             | -2.7            | -1.1             | 100            | 110           | 2/3          | 2/3         | Wow, ask me anything about voice there.    |
| Yep!    | -2.2             | -2.7            | -0.4             | 70             | 60            | 0/3          | 1/3         | Death ask me anything about voice there.   |
| Nope!   | -1.9             | -1.0            | 0.9              | 90             | 90            | 0/3          | 3/3         | Don't ask me anything about Vice.          |
| Cool!   | 0.3              | -1.2            | -1.5             | 10             | 80            | 1/3          | 1/3         | Who ask me anything about Voyster.         |
| Done!   | -0.3             | -1.2            | -0.9             | 20             | 90            | 1/3          | 1/3         | Done asked me anything about voice there.  |
| Got it! | -2.5             | -1.7            | 0.8              | 20             | 130           | 3/3          | 3/3         | Got it? Ask me anything about voice there. |

### `en-glados-high` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -0.8             | -2.1            | -1.3             | 0              | 0             | 0/3          | 0/3         | I ask me anything about voice there.       |
| Hey!    | -0.8             | -0.6            | 0.2              | 10             | 0             | 1/3          | 2/3         | Okay, ask me anything about whis there.    |
| Sure!   | -1.7             | -0.7            | 1.0              | 90             | 40            | 3/3          | 2/3         | Sure, ask me anything about Voice there.   |
| Yes!    | -1.9             | -1.3            | 0.7              | 10             | 20            | 3/3          | 2/3         | Yes, ask me anything about Voice there.    |
| No!     | -2.5             | -0.6            | 1.9              | 90             | 40            | 3/3          | 2/3         | No, ask me anything about voice there.     |
| Okay!   | -0.4             | -0.1            | 0.2              | 50             | 0             | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 0.0              | -0.3            | -0.3             | 20             | 0             | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | -3.4             | -1.5            | 1.9              | 70             | 40            | 3/3          | 1/3         | Great. Ask me anything about voice there.  |
| Thanks! | 0.8              | -2.0            | -2.7             | 90             | 20            | 3/3          | 3/3         | Thanks asked me anything about Voicare.    |
| Right!  | -3.2             | -2.8            | 0.5              | 50             | 30            | 3/3          | 3/3         | Right ask me anything about voice there.   |
| Hello!  | -3.5             | 1.1             | 4.7              | 40             | 50            | 0/3          | 3/3         | Ask me anything about voice there.         |
| Well!   | -3.3             | -1.6            | 1.7              | 80             | 30            | 2/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | -1.9             | -1.9            | 0.1              | 50             | 20            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | -2.0             | -1.9            | 0.1              | 10             | 20            | 2/3          | 1/3         | Oh, ask me anything of our voice there.    |
| Wow!    | -2.0             | -2.4            | -0.4             | 130            | 110           | 1/3          | 2/3         | Paul ask me anything about voice there.    |
| Yep!    | -1.2             | -2.1            | -0.9             | 60             | 50            | 0/3          | 0/3         | God ask me anything about voice there.     |
| Nope!   | -2.0             | -0.4            | 1.5              | 100            | 60            | 0/3          | 2/3         | No, ask me anything about voice there.     |
| Cool!   | 0.2              | -0.3            | -0.5             | 0              | 60            | 0/3          | 1/3         | Pool of ask me anything about voice there. |
| Done!   | -1.0             | -1.6            | -0.6             | 40             | 50            | 1/3          | 1/3         | Don ask me anything about voice there.     |
| Got it! | -4.2             | -1.9            | 2.3              | 50             | 80            | 2/3          | 0/3         | Got it? Ask me anything a baffles there.   |

### `en-ryan` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | -0.3             | 1.9             | 2.1              | 0              | 30            | 0/3          | 3/3         | Hey, ask me anything about voice there    |
| Hey!    | -0.2             | 2.1             | 2.3              | 0              | 60            | 3/3          | 3/3         | Hey, ask me anything about voice there    |
| Sure!   | -0.8             | 2.3             | 3.1              | 0              | 130           | 0/3          | 3/3         | Ask me anything about voice there         |
| Yes!    | -1.2             | -0.5            | 0.7              | 10             | 110           | 0/3          | 3/3         | This ask me anything about boys there     |
| No!     | 0.1              | 4.4             | 4.3              | 0              | 80            | 1/3          | 3/3         | No, ask me anything about voice there     |
| Okay!   | 2.4              | 3.4             | 1.0              | 20             | 140           | 2/3          | 3/3         | Opi ask me anything about voice there     |
| OK!     | 3.0              | 2.4             | -0.6             | 10             | 100           | 3/3          | 3/3         | Okay, ask me anything about boys there.   |
| Great!  | 0.4              | 0.9             | 0.6              | 20             | 130           | 2/3          | 3/3         | Great. Ask me anything about voice there  |
| Thanks! | -0.3             | 1.4             | 1.8              | 10             | 90            | 0/3          | 3/3         | Tence ask me anything about voice there.  |
| Right!  | -0.4             | 0.4             | 0.8              | 10             | 60            | 2/3          | 3/3         | Right, ask me anything about voice there  |
| Hello!  | 2.4              | 3.7             | 1.3              | 0              | 140           | 3/3          | 3/3         | Hello asked me anything about voice there |
| Well!   | 0.3              | 1.8             | 1.5              | 0              | 80            | 2/3          | 3/3         | Well, ask me anything about Voice there   |
| So!     | -0.1             | 3.8             | 3.9              | 0              | 20            | 2/3          | 3/3         | So ask me anything about voice there      |
| Oh!     | -0.6             | 2.7             | 3.3              | 0              | 50            | 0/3          | 3/3         | Ask me anything about voice there         |
| Wow!    | 0.2              | 2.4             | 2.2              | 0              | 100           | 0/3          | 3/3         | Well, ask me anything about voice there   |
| Yep!    | -0.2             | 2.2             | 2.4              | 10             | 80            | 0/3          | 2/3         | They'll ask me anything about voice there |
| Nope!   | -0.1             | 2.0             | 2.1              | 130            | 90            | 2/3          | 3/3         | Nope, ask me anything about voice there.  |
| Cool!   | -1.0             | -0.0            | 0.9              | 10             | 100           | 0/3          | 3/3         | who ask me anything about voice there     |
| Done!   | -0.5             | 2.8             | 3.3              | 10             | 40            | 0/3          | 2/3         | Then ask me anything about voice there    |
| Got it! | 0.9              | 0.7             | -0.1             | 20             | 150           | 2/3          | 3/3         | Got it? Ask me anything about Boys there. |

### `en-ryan` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)             |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------- |
| Hi!     | -0.9             | 1.1             | 2.0              | 0              | 20            | 0/3          | 3/3         | Ask me anything about voice there           |
| Hey!    | -0.1             | 2.4             | 2.5              | 10             | 70            | 2/3          | 3/3         | Hey, ask me anything about voice there      |
| Sure!   | -1.1             | 1.8             | 3.0              | 0              | 90            | 0/3          | 3/3         | to ask me anything about voice there        |
| Yes!    | -0.8             | -0.5            | 0.4              | 0              | 70            | 0/3          | 2/3         | is ask me anything about voice there        |
| No!     | 0.5              | 3.7             | 3.2              | 0              | 50            | 2/3          | 3/3         | No, ask me anything about voice there       |
| Okay!   | 0.5              | 2.6             | 2.1              | 0              | 160           | 2/3          | 3/3         | Obey, ask me anything about voice there     |
| OK!     | 2.9              | 2.6             | -0.3             | 10             | 100           | 2/3          | 3/3         | Okay, ask me anything about voice there     |
| Great!  | 0.1              | 1.6             | 1.5              | 30             | 100           | 2/3          | 3/3         | Great. Ask me anything about voice there    |
| Thanks! | 0.2              | 1.5             | 1.3              | 10             | 90            | 0/3          | 3/3         | Tense, ask me anything about voice there.   |
| Right!  | -0.1             | -0.3            | -0.2             | 20             | 60            | 1/3          | 3/3         | Bye, ask me anything about voice there      |
| Hello!  | 2.0              | 2.2             | 0.2              | 10             | 120           | 3/3          | 3/3         | Hello, ask me anything about voice there    |
| Well!   | 1.0              | 1.7             | 0.7              | 20             | 40            | 3/3          | 3/3         | Well, ask me anything about voice there     |
| So!     | -0.6             | 3.3             | 3.9              | 0              | 80            | 3/3          | 3/3         | So ask me anything about voice there        |
| Oh!     | -0.3             | 3.3             | 3.6              | 0              | 30            | 0/3          | 3/3         | Ask me anything about boys there            |
| Wow!    | 0.5              | 1.8             | 1.3              | 0              | 60            | 0/3          | 3/3         | Well, ask me anything about voice there     |
| Yep!    | 0.2              | 1.6             | 1.4              | 0              | 80            | 0/3          | 3/3         | Yeah, ask me anything about voice there     |
| Nope!   | -0.4             | 2.9             | 3.2              | 70             | 110           | 2/3          | 3/3         | Nope, ask me anything about voice there     |
| Cool!   | -0.6             | 1.2             | 1.8              | 0              | 90            | 0/3          | 3/3         | Ask me anything about voice there           |
| Done!   | -0.2             | 3.1             | 3.3              | 10             | 30            | 1/3          | 3/3         | Don't ask me anything about voice there     |
| Got it! | -0.3             | 0.5             | 0.9              | 30             | 120           | 1/3          | 3/3         | Dot it asked me anything about voice there. |

### `en-ryan-high` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)               |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | --------------------------------------------- |
| Hi!     | -0.1             | 1.2             | 1.3              | 0              | 0             | 0/3          | 3/3         | Ask me anything about voice there             |
| Hey!    | 0.6              | 1.9             | 1.2              | 10             | 0             | 0/3          | 3/3         | Ask me anything about voice there             |
| Sure!   | 1.7              | 2.3             | 0.6              | 10             | 170           | 0/3          | 3/3         | Good. Ask me anything about voice there       |
| Yes!    | 0.7              | -1.1            | -1.8             | 10             | 110           | 0/3          | 3/3         | Des asked me anything about voice there.      |
| No!     | 0.1              | 2.2             | 2.1              | 20             | 10            | 0/3          | 3/3         | Oh, ask me anything about voice there         |
| Okay!   | 2.4              | 1.7             | -0.6             | 20             | 170           | 3/3          | 3/3         | Okay, ask me anything about voice there       |
| OK!     | 1.7              | 1.0             | -0.6             | 10             | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there.      |
| Great!  | 0.6              | -0.7            | -1.3             | 10             | 70            | 3/3          | 3/3         | Great. Ask me anything about voice there.     |
| Thanks! | 0.0              | 0.2             | 0.2              | 20             | 130           | 2/3          | 3/3         | Thanks, ask me anything about voice there.    |
| Right!  | -0.1             | 0.8             | 1.0              | 10             | 20            | 0/3          | 3/3         | But ask me anything about voice there.        |
| Hello!  | 3.2              | 3.1             | -0.1             | 20             | 0             | 0/3          | 3/3         | Oh, asked me anything about voice there       |
| Well!   | 0.7              | 1.7             | 1.0              | 10             | 10            | 3/3          | 3/3         | Well, ask me anything about voice there       |
| So!     | 0.0              | 1.9             | 1.9              | 0              | 0             | 0/3          | 3/3         | Ask me anything about voice there             |
| Oh!     | -0.9             | 2.3             | 3.2              | 20             | 0             | 0/3          | 3/3         | Well, ask me anything about voice there.      |
| Wow!    | 0.1              | 2.0             | 1.9              | 10             | 10            | 0/3          | 2/3         | Well, ask me anything about voice there       |
| Yep!    | 1.7              | -0.9            | -2.6             | 20             | 70            | 0/3          | 3/3         | Hell ask me anything about voice there        |
| Nope!   | 0.5              | -0.0            | -0.6             | 20             | 60            | 0/3          | 3/3         | So ask me anything about voice there.         |
| Cool!   | 1.1              | 1.0             | -0.1             | 10             | 0             | 0/3          | 3/3         | Who ask me anything about voice there?        |
| Done!   | 0.9              | 1.7             | 0.8              | 10             | 30            | 0/3          | 1/3         | Ask me anything about voice there             |
| Got it! | 0.2              | 1.8             | 1.6              | 0              | 40            | 0/3          | 2/3         | Well, it asked me anything about voice there. |

### `en-ryan-high` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | 0.3              | 1.2             | 0.9              | 10             | 0             | 0/3          | 3/3         | Ask me anything about voice there              |
| Hey!    | 0.4              | 2.0             | 1.7              | 0              | 0             | 0/3          | 3/3         | Ask me anything about voice there.             |
| Sure!   | 0.4              | 2.2             | 1.8              | 10             | 130           | 0/3          | 3/3         | Go ask me anything about voice there           |
| Yes!    | 0.4              | 0.0             | -0.3             | 10             | 90            | 1/3          | 3/3         | Yes, ask me anything about voice there         |
| No!     | 0.5              | 2.2             | 1.6              | 10             | 0             | 0/3          | 3/3         | Oh, ask me anything about voice there          |
| Okay!   | 1.1              | 0.1             | -1.0             | 10             | 150           | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| OK!     | 2.5              | 0.7             | -1.8             | 10             | 140           | 3/3          | 3/3         | Okay, ask me anything about voice there.       |
| Great!  | 0.2              | -0.0            | -0.2             | 20             | 70            | 2/3          | 3/3         | Braith, ask me anything about voice there.     |
| Thanks! | 0.6              | -0.7            | -1.3             | 10             | 120           | 2/3          | 3/3         | Thanks! Ask me anything about voice there.     |
| Right!  | 0.0              | 1.0             | 0.9              | 10             | 20            | 0/3          | 2/3         | Well, if asked me anything about voice there   |
| Hello!  | 1.7              | 2.5             | 0.8              | 20             | 0             | 0/3          | 3/3         | Oh, well, ask me anything about voice there    |
| Well!   | 0.4              | 1.5             | 1.1              | 10             | 10            | 3/3          | 3/3         | Well, ask me anything about voice there        |
| So!     | 0.4              | 1.4             | 1.0              | 20             | 10            | 0/3          | 3/3         | Oh, ask me anything about voice there          |
| Oh!     | -0.7             | 1.9             | 2.6              | 10             | 0             | 1/3          | 3/3         | Oh, ask me anything about voice there          |
| Wow!    | -0.2             | 1.6             | 1.8              | 30             | 0             | 0/3          | 3/3         | Well, ask me anything about voice there        |
| Yep!    | 0.9              | -0.1            | -1.0             | 10             | 70            | 0/3          | 3/3         | They have asked me anything about voice there. |
| Nope!   | -0.1             | 0.6             | 0.7              | 20             | 40            | 0/3          | 3/3         | Though ask me anything about voice there       |
| Cool!   | 0.3              | 0.7             | 0.4              | 20             | 0             | 0/3          | 3/3         | Oh, ask me anything about voice there          |
| Done!   | 0.8              | 1.1             | 0.3              | 10             | 10            | 0/3          | 1/3         | Oh, ask me anything about voice there          |
| Got it! | 0.2              | 2.0             | 1.9              | 0              | 50            | 0/3          | 3/3         | about it ask me anything about voice there     |

### `en-ryan-low` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | -0.3             | -1.0            | -0.7             | 0              | 90            | 0/3          | 3/3         | Ah, ask me anything about voice there     |
| Hey!    | -0.7             | 0.3             | 1.0              | 30             | 90            | 0/3          | 3/3         | There, ask me anything about voice there. |
| Sure!   | 1.8              | 1.7             | -0.1             | 10             | 140           | 0/3          | 3/3         | Ask me anything about voice there.        |
| Yes!    | -2.3             | -2.6            | -0.3             | 0              | 90            | 0/3          | 3/3         | Beth asked me anything about voice there  |
| No!     | 0.1              | 1.1             | 1.0              | 20             | 120           | 1/3          | 3/3         | No, ask me anything about voice there     |
| Okay!   | 1.4              | -2.0            | -3.4             | 20             | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| OK!     | 1.4              | 1.1             | -0.4             | 20             | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| Great!  | -0.4             | 0.5             | 0.9              | 20             | 30            | 2/3          | 3/3         | Great. Ask me anything about voice there  |
| Thanks! | -1.8             | -0.7            | 1.1              | 0              | 40            | 2/3          | 3/3         | Great, ask me anything about voice there. |
| Right!  | 0.9              | -3.4            | -4.3             | 30             | 120           | 2/3          | 3/3         | Rive, ask me anything about voice there   |
| Hello!  | 0.5              | 2.9             | 2.4              | 20             | 170           | 3/3          | 3/3         | Hello ask me anything about voice there   |
| Well!   | 0.7              | 0.8             | 0.2              | 10             | 20            | 3/3          | 3/3         | Well, ask me anything about voice there.  |
| So!     | 1.1              | 0.7             | -0.4             | 20             | 90            | 0/3          | 3/3         | Oh, ask me anything about voice there.    |
| Oh!     | -0.5             | 0.2             | 0.7              | 20             | 140           | 2/3          | 3/3         | Uh, ask me anything about voice there.    |
| Wow!    | 0.7              | 1.8             | 1.1              | 20             | 150           | 0/3          | 1/3         | Well, ask me anything about voice there.  |
| Yep!    | -1.2             | 1.0             | 2.2              | 20             | 110           | 0/3          | 1/3         | Yeah, ask me anything about voice there   |
| Nope!   | -0.3             | 0.9             | 1.2              | 40             | 40            | 0/3          | 2/3         | Nove asked me anything about voice there. |
| Cool!   | 2.2              | -0.3            | -2.5             | 0              | 70            | 0/3          | 3/3         | Girl asked me anything about voice there. |
| Done!   | -0.6             | -0.4            | 0.2              | 20             | 100           | 0/3          | 2/3         | Ask me anything about voice there.        |
| Got it! | 0.3              | 0.1             | -0.2             | 30             | 40            | 2/3          | 2/3         | Got it ask me anything about voice there. |

### `en-ryan-low` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -0.4             | -0.7            | -0.3             | 0              | 110           | 0/3          | 3/3         | I ask me anything about voice there        |
| Hey!    | -0.1             | -0.6            | -0.5             | 20             | 90            | 0/3          | 3/3         | Bef ask me anything about voice there      |
| Sure!   | 1.9              | 0.4             | -1.5             | 20             | 110           | 0/3          | 3/3         | Do her asked me anything about voice there |
| Yes!    | -2.9             | -2.6            | 0.3              | 10             | 100           | 0/3          | 3/3         | Ask me anything about voice there.         |
| No!     | 0.5              | 0.4             | -0.1             | 20             | 150           | 1/3          | 3/3         | Hell ask me anything about voice there.    |
| Okay!   | 0.6              | -2.9            | -3.5             | 10             | 160           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 1.5              | 0.4             | -1.1             | 20             | 160           | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | -0.9             | 0.2             | 1.1              | 30             | 120           | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | -1.1             | -0.1            | 1.0              | 20             | 150           | 2/3          | 3/3         | Eggs asked me anything about voice there   |
| Right!  | 1.2              | -2.9            | -4.2             | 30             | 110           | 1/3          | 3/3         | Right? Ask me anything about voice there   |
| Hello!  | 0.7              | 1.4             | 0.7              | 10             | 90            | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 0.2              | 1.5             | 1.3              | 30             | 30            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 1.2              | 0.1             | -1.0             | 20             | 150           | 0/3          | 3/3         | Though ask me anything about voice there   |
| Oh!     | 1.7              | -0.5            | -2.2             | 20             | 140           | 1/3          | 3/3         | Ask me anything about voice there.         |
| Wow!    | 1.4              | 0.9             | -0.4             | 20             | 90            | 0/3          | 2/3         | Well, ask me anything about voice there.   |
| Yep!    | -1.1             | -0.7            | 0.4              | 10             | 80            | 0/3          | 2/3         | ask me anything about voice there          |
| Nope!   | 0.4              | 0.5             | 0.1              | 10             | 40            | 0/3          | 3/3         | No, ask me anything about voice there.     |
| Cool!   | 2.1              | 0.6             | -1.5             | 30             | 120           | 0/3          | 2/3         | Go ask me anything about voice there.      |
| Done!   | -1.0             | 0.1             | 1.1              | 20             | 50            | 0/3          | 2/3         | God, ask me anything about voice there     |
| Got it! | -0.8             | -0.4            | 0.4              | 10             | 30            | 1/3          | 3/3         | Got it? Ask me anything about voice there  |

### `en-joe` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 2.5              | 6.1             | 3.6              | 210            | 40            | 0/3          | 2/3         | I ask me anything about voice there.       |
| Hey!    | 5.3              | 4.1             | -1.2             | 80             | 50            | 2/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 4.2              | 4.8             | 0.5              | 70             | 40            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 2.8              | 7.6             | 4.8              | 190            | 50            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 3.5              | 6.7             | 3.2              | 190            | 40            | 2/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.7              | 5.6             | 2.9              | 70             | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 3.0              | 3.7             | 0.8              | 80             | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 4.5              | 5.3             | 0.8              | 80             | 70            | 2/3          | 3/3         | Grey asked me anything about voice there.  |
| Thanks! | 4.9              | 5.4             | 0.5              | 220            | 40            | 3/3          | 3/3         | Thanks, ask me anything about voice there. |
| Right!  | 5.9              | 7.2             | 1.3              | 210            | 60            | 2/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 3.8              | 7.5             | 3.7              | 180            | 40            | 3/3          | 3/3         | Hello, ask me anything about voice there.  |
| Well!   | 6.0              | 7.4             | 1.5              | 70             | 40            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 5.6              | 7.8             | 2.2              | 80             | 40            | 3/3          | 3/3         | So ask me anything about boys there.       |
| Oh!     | 4.5              | 6.0             | 1.5              | 160            | 40            | 2/3          | 3/3         | Oh, ask me anything about Boyce there.     |
| Wow!    | 4.6              | 7.0             | 2.5              | 210            | 40            | 2/3          | 0/3         | While ask me anything about voice there.   |
| Yep!    | 6.0              | 6.6             | 0.5              | 160            | 70            | 0/3          | 0/3         | Yet ask me anything about voice there.     |
| Nope!   | 3.3              | 5.3             | 2.1              | 190            | 60            | 3/3          | 1/3         | Nope, ask me anything about voice there.   |
| Cool!   | 4.7              | 4.6             | -0.2             | 80             | 40            | 0/3          | 2/3         | Goo asked me anything about voice there.   |
| Done!   | 2.6              | 5.3             | 2.7              | 190            | 50            | 0/3          | 2/3         | Don asked me anything about voice there.   |
| Got it! | 4.5              | 8.3             | 3.8              | 220            | 50            | 2/3          | 0/3         | Ask me anything about voice there.         |

### `en-joe` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 2.2              | 6.4             | 4.1              | 200            | 40            | 0/3          | 3/3         | I ask me anything about voice there.      |
| Hey!    | 4.2              | 3.9             | -0.3             | 70             | 40            | 2/3          | 3/3         | Hey, ask me anything about voice there.   |
| Sure!   | 4.4              | 3.9             | -0.4             | 70             | 40            | 3/3          | 3/3         | Sure, ask me anything about voice there.  |
| Yes!    | 2.5              | 6.5             | 4.0              | 170            | 40            | 3/3          | 3/3         | Yes, ask me anything about voice there.   |
| No!     | 3.5              | 6.8             | 3.3              | 190            | 40            | 3/3          | 3/3         | No, ask me anything about voice there.    |
| Okay!   | 3.9              | 5.4             | 1.5              | 80             | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| OK!     | 2.5              | 4.8             | 2.3              | 80             | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| Great!  | 4.3              | 4.6             | 0.3              | 70             | 50            | 3/3          | 3/3         | Great, ask me anything about voice there. |
| Thanks! | 3.8              | 5.5             | 1.7              | 200            | 40            | 3/3          | 3/3         | Thanks ask me anything about voice there. |
| Right!  | 5.6              | 6.5             | 1.0              | 90             | 60            | 2/3          | 3/3         | Right, ask me anything about voice there. |
| Hello!  | 4.1              | 6.5             | 2.4              | 160            | 50            | 3/3          | 3/3         | Hello ask me anything about voice there   |
| Well!   | 4.0              | 7.0             | 3.0              | 210            | 40            | 3/3          | 3/3         | Well, ask me anything about voice there.  |
| So!     | 3.0              | 7.6             | 4.6              | 210            | 70            | 3/3          | 3/3         | So ask me anything about voice there.     |
| Oh!     | 5.2              | 4.8             | -0.4             | 140            | 40            | 3/3          | 3/3         | Oh, ask me anything about voice there.    |
| Wow!    | 4.7              | 6.6             | 2.0              | 200            | 40            | 3/3          | 0/3         | Wow, ask me anything about voice there.   |
| Yep!    | 5.1              | 6.7             | 1.6              | 160            | 50            | 0/3          | 0/3         | Yet ask me anything about voice there.    |
| Nope!   | 4.5              | 4.6             | 0.1              | 170            | 60            | 2/3          | 0/3         | Nope, ask me anything about voice there.  |
| Cool!   | 3.5              | 3.5             | 0.0              | 80             | 30            | 0/3          | 2/3         | Go asked me anything about voice there.   |
| Done!   | 3.8              | 4.2             | 0.4              | 190            | 50            | 1/3          | 1/3         | Done ask me anything about voice there.   |
| Got it! | 3.9              | 7.5             | 3.5              | 220            | 50            | 2/3          | 2/3         | At it, ask me anything about voice there. |

### `en-bryce` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 1.3              | 1.2             | -0.1             | 130            | 20            | 3/3          | 0/3         | Hi, ask me anything about voice there     |
| Hey!    | 2.2              | 1.9             | -0.3             | 170            | 110           | 3/3          | 3/3         | Hey ask me anything about voice there     |
| Sure!   | 8.2              | 5.7             | -2.6             | 110            | 80            | 3/3          | 3/3         | Sure ask me anything about voice there.   |
| Yes!    | 2.6              | 2.8             | 0.2              | 120            | 110           | 0/3          | 2/3         | Des ask me anything about voice there     |
| No!     | 4.4              | 4.6             | 0.3              | 130            | 50            | 3/3          | 3/3         | No ask me anything about voice there      |
| Okay!   | 3.2              | 2.9             | -0.3             | 60             | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| OK!     | 4.6              | 4.2             | -0.4             | 60             | 100           | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | 2.9              | 5.6             | 2.7              | 80             | 160           | 3/3          | 3/3         | Great Ask me anything about voice there   |
| Thanks! | 3.4              | 1.6             | -1.8             | 170            | 160           | 3/3          | 3/3         | Thanks ask me anything about voice there  |
| Right!  | 2.5              | 2.6             | 0.1              | 40             | 90            | 3/3          | 0/3         | Right, ask me anything about voice there  |
| Hello!  | 6.0              | 6.1             | 0.1              | 170            | 70            | 3/3          | 1/3         | Hello, ask me anything about voice there  |
| Well!   | 2.8              | 4.1             | 1.4              | 90             | 50            | 3/3          | 3/3         | Well, ask me anything about voice there   |
| So!     | 4.6              | 4.0             | -0.6             | 100            | 50            | 3/3          | 3/3         | So ask me anything about voice there      |
| Oh!     | 3.1              | 3.9             | 0.9              | 100            | 50            | 3/3          | 1/3         | Oh ask me anything about voice there      |
| Wow!    | 4.8              | 1.4             | -3.4             | 140            | 40            | 3/3          | 3/3         | Wow ask me anything about voice there     |
| Yep!    | 1.1              | 0.5             | -0.6             | 120            | 80            | 1/3          | 3/3         | Yep ask me anything about voice there     |
| Nope!   | 3.8              | 3.0             | -0.8             | 60             | 100           | 3/3          | 3/3         | Nope ask me anything about voice there    |
| Cool!   | 3.4              | 3.1             | -0.2             | 110            | 60            | 3/3          | 3/3         | Cool asked me anything about voice there. |
| Done!   | 1.9              | 0.8             | -1.1             | 60             | 90            | 0/3          | 3/3         | Bon asked me anything about voice there   |
| Got it! | 1.2              | 1.6             | 0.4              | 90             | 130           | 0/3          | 0/3         | Athit ask me anything about voice there.  |

### `en-bryce` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 2.4              | 1.4             | -1.0             | 120            | 30            | 3/3          | 0/3         | Hi ask me anything about voice there      |
| Hey!    | 1.7              | 1.8             | 0.1              | 180            | 70            | 3/3          | 2/3         | Hey ask me anything about voice there     |
| Sure!   | 7.9              | 4.6             | -3.2             | 100            | 80            | 3/3          | 3/3         | Sure, ask me anything about voice there   |
| Yes!    | 0.9              | 2.6             | 1.8              | 120            | 100           | 2/3          | 1/3         | Yes, ask me anything about voice there    |
| No!     | 3.9              | 4.0             | 0.1              | 80             | 40            | 3/3          | 3/3         | No ask me anything about voice there      |
| Okay!   | 3.2              | 3.2             | 0.1              | 60             | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| OK!     | 4.8              | 3.6             | -1.2             | 210            | 110           | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | 2.4              | 5.6             | 3.2              | 60             | 140           | 3/3          | 2/3         | Great Ask me anything about voice there   |
| Thanks! | 2.5              | 1.0             | -1.5             | 140            | 120           | 3/3          | 3/3         | Thanks ask me anything about voice there. |
| Right!  | 1.8              | 2.8             | 1.0              | 50             | 90            | 3/3          | 0/3         | Right, ask me anything about voice there  |
| Hello!  | 6.2              | 5.9             | -0.3             | 170            | 70            | 3/3          | 0/3         | Hello ask me anything about voice there   |
| Well!   | 2.8              | 3.7             | 0.9              | 100            | 40            | 3/3          | 3/3         | Well ask me anything about voice there    |
| So!     | 4.2              | 3.7             | -0.5             | 120            | 60            | 3/3          | 3/3         | So ask me anything about voice there      |
| Oh!     | 3.5              | 2.9             | -0.6             | 90             | 30            | 3/3          | 0/3         | Oh, ask me anything about voice there     |
| Wow!    | 4.7              | 1.7             | -3.0             | 110            | 30            | 3/3          | 3/3         | Wow ask me anything about voice there     |
| Yep!    | 1.6              | 1.2             | -0.4             | 90             | 80            | 1/3          | 3/3         | Yep asked me anything about voice there   |
| Nope!   | 2.5              | 3.2             | 0.7              | 60             | 80            | 3/3          | 3/3         | Nope ask me anything about voice there    |
| Cool!   | 2.9              | 3.2             | 0.3              | 110            | 70            | 2/3          | 3/3         | Cool. Ask me anything about voice there   |
| Done!   | 1.5              | 0.9             | -0.6             | 50             | 110           | 0/3          | 3/3         | Bun asked me anything about voice there   |
| Got it! | 0.3              | 1.2             | 0.9              | 80             | 150           | 0/3          | 0/3         | Atte ask me anything about voice there    |

### `en-john` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -0.9             | 1.5             | 2.4              | 110            | 50            | 2/3          | 2/3         | I ask me anything about voice there        |
| Hey!    | -2.6             | 1.2             | 3.8              | 50             | 40            | 2/3          | 1/3         | Hey, ask me anything about voice there.    |
| Sure!   | -3.0             | 1.3             | 4.3              | 50             | 40            | 3/3          | 3/3         | Sure, ask me anything about voice there    |
| Yes!    | -4.9             | 2.7             | 7.5              | 190            | 70            | 3/3          | 3/3         | Yes, ask me anything about voice there     |
| No!     | -0.6             | 4.7             | 5.3              | 200            | 60            | 0/3          | 3/3         | Oh, ask me anything about voice there      |
| Okay!   | -1.7             | 2.0             | 3.7              | 50             | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | -0.6             | 2.0             | 2.6              | 40             | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | -5.1             | -1.9            | 3.3              | 170            | 130           | 1/3          | 3/3         | Right. Ask me anything about voice there   |
| Thanks! | -1.5             | -1.6            | -0.0             | 150            | 100           | 3/3          | 3/3         | Thanks ask me anything about voice there.  |
| Right!  | -3.9             | 0.7             | 4.6              | 120            | 90            | 3/3          | 3/3         | Right, ask me anything about voice there   |
| Hello!  | 1.3              | 5.9             | 4.6              | 150            | 80            | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | -0.2             | 3.2             | 3.4              | 140            | 20            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 1.5              | 1.0             | -0.5             | 190            | 160           | 3/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | -1.4             | 2.2             | 3.6              | 160            | 10            | 1/3          | 2/3         | Well, ask me anything about voice there    |
| Wow!    | -1.4             | 0.9             | 2.3              | 200            | 70            | 3/3          | 1/3         | Wow, ask me anything about voice there     |
| Yep!    | -1.3             | 1.0             | 2.3              | 80             | 70            | 0/3          | 1/3         | Ask me anything about voice there.         |
| Nope!   | 0.1              | 3.8             | 3.6              | 160            | 110           | 0/3          | 1/3         | No, ask me anything about voice there      |
| Cool!   | -0.1             | -1.2            | -1.1             | 70             | 40            | 0/3          | 1/3         | He will ask me anything about voice there. |
| Done!   | 1.9              | 2.7             | 0.8              | 200            | 50            | 0/3          | 0/3         | An asked me anything about voice there     |
| Got it! | 0.6              | 1.8             | 1.2              | 190            | 120           | 0/3          | 0/3         | but Q me anything about voice there        |

### `en-john` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)              |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | -------------------------------------------- |
| Hi!     | -1.7             | 0.9             | 2.6              | 170            | 40            | 3/3          | 1/3         | Hi, ask me anything about Voice there        |
| Hey!    | -3.4             | 1.8             | 5.1              | 150            | 40            | 3/3          | 3/3         | Hey, ask me anything about voice there       |
| Sure!   | -5.0             | 0.9             | 5.9              | 160            | 40            | 3/3          | 3/3         | Sure, ask me anything about voice there.     |
| Yes!    | -5.9             | 2.3             | 8.2              | 160            | 90            | 2/3          | 3/3         | Yes, ask me anything about voice there.      |
| No!     | -2.9             | 3.9             | 6.7              | 120            | 50            | 2/3          | 3/3         | Oh, ask me anything about voice there        |
| Okay!   | -5.1             | 1.9             | 7.0              | 170            | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there      |
| OK!     | -1.1             | 3.6             | 4.7              | 110            | 110           | 3/3          | 3/3         | Okay, ask me anything about voice there.     |
| Great!  | -4.0             | -3.6            | 0.4              | 130            | 100           | 1/3          | 3/3         | Right, ask me anything about voice there     |
| Thanks! | -0.7             | -0.8            | -0.1             | 170            | 70            | 3/3          | 3/3         | Thanks, ask me anything about voice there    |
| Right!  | -4.3             | 1.7             | 6.0              | 130            | 70            | 3/3          | 3/3         | Right, ask me anything about voice there     |
| Hello!  | 2.7              | 5.4             | 2.7              | 160            | 50            | 2/3          | 3/3         | Hello Ask me anything about voice there      |
| Well!   | 0.1              | 1.7             | 1.5              | 170            | 30            | 3/3          | 3/3         | Well, ask me anything about voice there.     |
| So!     | 1.8              | 3.4             | 1.6              | 180            | 40            | 3/3          | 3/3         | So, ask me anything about voice there        |
| Oh!     | -6.2             | 1.1             | 7.4              | 160            | 60            | 3/3          | 3/3         | Oh, ask me anything about voice there.       |
| Wow!    | -3.6             | 1.4             | 5.0              | 180            | 30            | 3/3          | 2/3         | Wow, ask me anything about voice there.      |
| Yep!    | 0.7              | 1.8             | 1.1              | 90             | 60            | 0/3          | 0/3         | They have ask me anything about voice there. |
| Nope!   | -0.6             | 4.4             | 5.0              | 100            | 80            | 0/3          | 2/3         | No, ask me anything about voice there.       |
| Cool!   | 0.3              | 0.6             | 0.3              | 150            | 50            | 2/3          | 0/3         | to ask me anything about voice there         |
| Done!   | -0.4             | 1.8             | 2.2              | 160            | 50            | 0/3          | 0/3         | An, ask me anything about voice there.       |
| Got it! | -0.8             | 1.7             | 2.5              | 200            | 120           | 0/3          | 0/3         | Art it? Ask me anything about voice there.   |

### `en-kristin` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | 2.7              | -1.6            | -4.3             | 160            | 80            | 3/3          | 3/3         | Hi ask me anything about Voice bear.           |
| Hey!    | 1.4              | -1.9            | -3.3             | 180            | 70            | 3/3          | 3/3         | Hey ask me anything about voice there.         |
| Sure!   | 1.7              | 0.1             | -1.5             | 180            | 70            | 3/3          | 3/3         | Sure, ask me anything about voice there.       |
| Yes!    | -3.4             | -1.4            | 1.9              | 210            | 70            | 3/3          | 3/3         | Yes, ask me anything about voice there         |
| No!     | 2.4              | 0.3             | -2.1             | 190            | 90            | 3/3          | 3/3         | No ask me anything about voice there.          |
| Okay!   | 0.8              | -2.0            | -2.7             | 180            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there.       |
| OK!     | 1.9              | -0.9            | -2.8             | 190            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| Great!  | -4.4             | -0.4            | 4.0              | 190            | 80            | 3/3          | 1/3         | Great ask me anything about voice there.       |
| Thanks! | 0.6              | -1.3            | -1.8             | 170            | 20            | 3/3          | 3/3         | Thanks. Ask me anything about voice there      |
| Right!  | 0.6              | -1.6            | -2.1             | 160            | 50            | 3/3          | 3/3         | Right, ask me anything about voice there       |
| Hello!  | 5.8              | 0.8             | -5.0             | 180            | 60            | 1/3          | 3/3         | Hello Ask me anything about voice there        |
| Well!   | 1.6              | 0.1             | -1.5             | 150            | 60            | 3/3          | 3/3         | Well, ask me anything about voice there.       |
| So!     | 3.4              | -0.3            | -3.7             | 160            | 60            | 3/3          | 3/3         | So ask me anything about voice there.          |
| Oh!     | 1.9              | 0.9             | -1.0             | 170            | 70            | 3/3          | 0/3         | Oh, ask me anything of a voice there           |
| Wow!    | 2.6              | 0.1             | -2.5             | 190            | 50            | 2/3          | 1/3         | Whoa, ask me anything about voice there.       |
| Yep!    | -0.2             | -0.9            | -0.8             | 90             | 70            | 2/3          | 3/3         | Yet ask me anything about voice there.         |
| Nope!   | 2.3              | -1.8            | -4.2             | 120            | 90            | 3/3          | 2/3         | Nope ask me anything above voice there.        |
| Cool!   | 0.9              | -0.8            | -1.7             | 170            | 60            | 2/3          | 1/3         | Cool asked me anything about voice there.      |
| Done!   | 1.0              | -1.3            | -2.2             | 180            | 90            | 1/3          | 2/3         | Tun ask me anything about voice there          |
| Got it! | 0.9              | -2.2            | -3.0             | 200            | 90            | 0/3          | 0/3         | thought it, ask me anything about voice there. |

### `en-kristin` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | 2.8              | -1.5            | -4.3             | 170            | 60            | 3/3          | 3/3         | Hi, ask me anything about voice there.         |
| Hey!    | -0.2             | -1.0            | -0.8             | 160            | 50            | 3/3          | 3/3         | Hey, ask me anything about voice there.        |
| Sure!   | 2.3              | -0.8            | -3.1             | 190            | 80            | 3/3          | 3/3         | Sure, ask me anything about voice there.       |
| Yes!    | -2.7             | -1.6            | 1.1              | 180            | 80            | 3/3          | 3/3         | Yes, ask me anything about voice there.        |
| No!     | 2.2              | -1.1            | -3.3             | 150            | 30            | 3/3          | 1/3         | No, ask me anything about voice there.         |
| Okay!   | 1.2              | -3.5            | -4.7             | 170            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there.       |
| OK!     | 2.0              | -0.8            | -2.8             | 150            | 150           | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| Great!  | -4.7             | -2.1            | 2.6              | 150            | 100           | 2/3          | 1/3         | Great, ask me anything about voice there.      |
| Thanks! | -0.3             | -1.1            | -0.7             | 170            | 50            | 3/3          | 3/3         | Thanks. Ask me anything about voice there.     |
| Right!  | -0.1             | -1.7            | -1.6             | 140            | 50            | 3/3          | 3/3         | Right, ask me anything about voice there.      |
| Hello!  | 4.7              | 0.6             | -4.1             | 150            | 70            | 2/3          | 3/3         | Hello ask me anything about voice there        |
| Well!   | 0.9              | 0.5             | -0.4             | 150            | 40            | 3/3          | 3/3         | Well, ask me anything about voice there.       |
| So!     | 3.0              | -0.3            | -3.3             | 170            | 50            | 3/3          | 3/3         | So ask me anything about voice there.          |
| Oh!     | 1.5              | -0.4            | -1.9             | 190            | 20            | 3/3          | 0/3         | Oh, ask me anything about voice there.         |
| Wow!    | 2.7              | -0.2            | -3.0             | 180            | 40            | 3/3          | 1/3         | Wow, ask me anything about voice there.        |
| Yep!    | -0.1             | -1.8            | -1.7             | 90             | 50            | 3/3          | 3/3         | Yep, ask me anything about voice there.        |
| Nope!   | 0.9              | -1.4            | -2.3             | 80             | 70            | 0/3          | 2/3         | Dope ask me anything about voice there.        |
| Cool!   | 0.5              | -0.5            | -0.9             | 150            | 30            | 2/3          | 3/3         | Who asked me anything about voice there?       |
| Done!   | 1.0              | -1.0            | -2.1             | 150            | 60            | 2/3          | 1/3         | Done, ask me anything about voice there.       |
| Got it! | 0.9              | -2.2            | -3.1             | 180            | 80            | 0/3          | 0/3         | thought it, ask me anything about voice there. |

### `en-norman` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -7.6             | -0.4            | 7.2              | 30             | 70            | 0/3          | 2/3         | Ask me anything about voice there          |
| Hey!    | -5.0             | -3.0            | 2.0              | 50             | 70            | 0/3          | 3/3         | They ask me anything about voice there     |
| Sure!   | 2.1              | 0.2             | -1.8             | 90             | 80            | 0/3          | 3/3         | to ask me anything about voice there       |
| Yes!    | -2.1             | -1.4            | 0.7              | 150            | 40            | 1/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | -1.3             | -1.5            | -0.2             | 110            | 70            | 1/3          | 2/3         | Now ask me anything about voice there.     |
| Okay!   | 0.8              | -1.2            | -2.0             | 70             | 70            | 0/3          | 3/3         | to ask me anything about voice there       |
| OK!     | 2.8              | -0.5            | -3.3             | 100            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | -3.2             | -1.0            | 2.2              | 100            | 90            | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | -3.8             | -2.3            | 1.5              | 160            | 110           | 2/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | -0.8             | -2.4            | -1.6             | 40             | 80            | 3/3          | 2/3         | Right. Ask me anything about voice there   |
| Hello!  | 0.8              | -2.1            | -2.8             | 120            | 70            | 1/3          | 2/3         | Hello, ask me anything about voicelair     |
| Well!   | -1.1             | -1.2            | -0.1             | 130            | 150           | 3/3          | 3/3         | Well, ask me anything about voice there    |
| So!     | 0.5              | -2.0            | -2.5             | 140            | 30            | 2/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | -2.4             | -1.5            | 0.9              | 130            | 60            | 0/3          | 0/3         | Ask me anything about voice there.         |
| Wow!    | -1.9             | 1.6             | 3.5              | 110            | 50            | 0/3          | 2/3         | Well, ask me anything about voice there    |
| Yep!    | 3.4              | -1.8            | -5.2             | 150            | 60            | 0/3          | 3/3         | the ask me anything about voice there      |
| Nope!   | -1.2             | -0.5            | 0.7              | 150            | 120           | 0/3          | 0/3         | Note. Ask me anything about voice there    |
| Cool!   | 0.0              | -1.6            | -1.7             | 80             | 70            | 0/3          | 3/3         | who ask me anything about voice there.     |
| Done!   | 2.3              | -1.4            | -3.6             | 120            | 40            | 0/3          | 2/3         | Diselle ask me anything about voice there  |
| Got it! | -2.8             | 0.8             | 3.5              | 200            | 110           | 0/3          | 0/3         | That it. Ask me anything about voice there |

### `en-norman` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -4.9             | -1.4            | 3.5              | 40             | 140           | 0/3          | 3/3         | Ask me anything about voice there          |
| Hey!    | -8.2             | -2.0            | 6.1              | 80             | 80            | 0/3          | 3/3         | Ask me anything about voice there          |
| Sure!   | 2.3              | -0.1            | -2.4             | 150            | 110           | 0/3          | 3/3         | Doyor asked me anything about voice there. |
| Yes!    | -3.5             | -1.3            | 2.2              | 100            | 80            | 0/3          | 3/3         | Ask me anything about voice there          |
| No!     | -2.1             | -0.1            | 2.0              | 110            | 70            | 2/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 1.4              | -1.7            | -3.1             | 70             | 50            | 1/3          | 3/3         | Do they ask me anything about voice there  |
| OK!     | 1.6              | -0.2            | -1.8             | 90             | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | -3.7             | -0.6            | 3.1              | 140            | 90            | 2/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | -4.3             | -3.3            | 1.0              | 200            | 60            | 2/3          | 3/3         | things ask me anything about voice there   |
| Right!  | -2.4             | -1.3            | 1.1              | 40             | 90            | 3/3          | 3/3         | right ask me anything about voice there    |
| Hello!  | 2.1              | -1.3            | -3.4             | 180            | 50            | 0/3          | 2/3         | Alt ask me anything about voice there      |
| Well!   | -1.7             | 0.7             | 2.4              | 150            | 100           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | -0.6             | -0.6            | 0.1              | 140            | 60            | 2/3          | 3/3         | Though ask me anything about voice there   |
| Oh!     | -4.0             | -1.8            | 2.2              | 80             | 150           | 0/3          | 0/3         | Ask me anything about voice there.         |
| Wow!    | -2.2             | 0.7             | 2.8              | 80             | 60            | 0/3          | 2/3         | Well, ask me anything about voice there.   |
| Yep!    | 2.3              | -1.0            | -3.3             | 90             | 70            | 0/3          | 0/3         | Yeah, ask me anything about voice there    |
| Nope!   | -0.8             | -0.4            | 0.4              | 150            | 110           | 0/3          | 1/3         | No. Ask me anything about voice there.     |
| Cool!   | -1.6             | -3.0            | -1.4             | 140            | 110           | 0/3          | 3/3         | Who asked me anything about voice there?   |
| Done!   | 1.6              | -0.9            | -2.4             | 160            | 30            | 0/3          | 2/3         | that one ask me anything about voice there |
| Got it! | -3.5             | 0.2             | 3.7              | 180            | 50            | 0/3          | 3/3         | that ask the anything about voice there    |

### `en-danny-low` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)              |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | -------------------------------------------- |
| Hi!     | 0.3              | -2.5            | -2.7             | 10             | 0             | 1/3          | 0/3         | I ask me anything about voice there.         |
| Hey!    | -0.2             | -0.7            | -0.5             | 30             | 10            | 3/3          | 3/3         | Hey, ask me anything about voice there.      |
| Sure!   | 3.2              | -0.1            | -3.3             | 150            | 0             | 0/3          | 3/3         | Shell asked me anything about voice there.   |
| Yes!    | 2.4              | -1.8            | -4.3             | 140            | 90            | 0/3          | 3/3         | ask me anything about voice there.           |
| No!     | 1.3              | -2.9            | -4.3             | 140            | 30            | 0/3          | 0/3         | Now ask me anything about voice there.       |
| Okay!   | 3.2              | -1.9            | -5.1             | 50             | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there.     |
| OK!     | 3.9              | -1.1            | -5.0             | 30             | 30            | 3/3          | 3/3         | Okay, ask me anything about voice there.     |
| Great!  | 1.3              | 1.8             | 0.5              | 130            | 20            | 3/3          | 3/3         | Great asked me anything about voice there.   |
| Thanks! | 1.0              | -6.6            | -7.5             | 140            | 150           | 2/3          | 3/3         | Angx asked me anything about voice there.    |
| Right!  | -0.3             | -1.0            | -0.7             | 30             | 10            | 0/3          | 2/3         | Light ask me anything about voice there.     |
| Hello!  | 2.1              | -0.5            | -2.6             | 140            | 10            | 3/3          | 3/3         | Hello ask me anything about voice there.     |
| Well!   | 4.4              | 1.2             | -3.2             | 30             | 0             | 2/3          | 3/3         | Bill ask me anything about voice there.      |
| So!     | 1.7              | -1.9            | -3.6             | 160            | 30            | 2/3          | 3/3         | So ask me anything about voice there.        |
| Oh!     | 2.3              | -1.6            | -4.0             | 90             | 20            | 0/3          | 0/3         | O asked me anything about voice there.       |
| Wow!    | 2.4              | -3.2            | -5.6             | 30             | 0             | 0/3          | 3/3         | Thou ask me anything about voice there.      |
| Yep!    | 2.9              | -2.9            | -5.8             | 130            | 10            | 0/3          | 0/3         | If ask me anything about voice there.        |
| Nope!   | 0.9              | -2.2            | -3.1             | 20             | 0             | 0/3          | 0/3         | asked me anything about voice there.         |
| Cool!   | 1.4              | -0.9            | -2.3             | 130            | 0             | 0/3          | 0/3         | Pool asked me anything about voice there.    |
| Done!   | -0.3             | -3.8            | -3.5             | 170            | 20            | 1/3          | 2/3         | Done ask me anything about voice there.      |
| Got it! | 3.1              | 1.3             | -1.8             | 190            | 40            | 0/3          | 0/3         | Bart it asked me anything about voice there. |

### `en-danny-low` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)             |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------- |
| Hi!     | -0.6             | -3.1            | -2.4             | 170            | 20            | 2/3          | 2/3         | Hi ask me anything about voice there.       |
| Hey!    | 0.2              | -0.7            | -0.8             | 150            | 100           | 3/3          | 3/3         | Hey, ask me anything about voice there.     |
| Sure!   | 2.7              | -1.3            | -4.0             | 150            | 90            | 0/3          | 3/3         | Show ask me anything about voice there.     |
| Yes!    | 2.7              | -2.5            | -5.2             | 120            | 100           | 0/3          | 3/3         | Des asked me anything about voice there.    |
| No!     | 1.8              | -2.5            | -4.3             | 100            | 30            | 2/3          | 2/3         | Now ask me anything about voice there.      |
| Okay!   | 3.9              | -2.8            | -6.7             | 20             | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there.    |
| OK!     | 4.0              | -0.7            | -4.7             | 140            | 30            | 3/3          | 3/3         | Okay, asked me anything about voice there.  |
| Great!  | 1.7              | 1.9             | 0.3              | 130            | 30            | 3/3          | 2/3         | Great ask me anything about voice there.    |
| Thanks! | 0.5              | -5.0            | -5.5             | 130            | 140           | 2/3          | 3/3         | Thanks, ask me anything about voice there.  |
| Right!  | -0.2             | -2.4            | -2.2             | 140            | 0             | 0/3          | 2/3         | I ask me anything about voice there.        |
| Hello!  | 2.3              | -1.0            | -3.3             | 150            | 20            | 3/3          | 3/3         | Hello asked me anything about voice there.  |
| Well!   | 3.7              | 1.2             | -2.5             | 130            | 0             | 2/3          | 3/3         | Well, ask me anything about voice there.    |
| So!     | 2.8              | -0.9            | -3.6             | 140            | 100           | 2/3          | 3/3         | So ask me anything about voice there.       |
| Oh!     | -0.2             | -2.1            | -2.0             | 80             | 80            | 0/3          | 1/3         | El ask me anything about voice there.       |
| Wow!    | 1.9              | -3.4            | -5.3             | 40             | 10            | 0/3          | 2/3         | Pau ask me anything about voice there.      |
| Yep!    | 2.3              | -2.9            | -5.1             | 120            | 0             | 1/3          | 0/3         | Yep ask me anything about voice there.      |
| Nope!   | 1.8              | -2.8            | -4.6             | 10             | 0             | 0/3          | 1/3         | Deb asked me anything about voice there.    |
| Cool!   | 1.9              | -1.6            | -3.5             | 120            | 0             | 0/3          | 0/3         | Will asked me anything about voice there.   |
| Done!   | 0.2              | -5.2            | -5.4             | 150            | 140           | 1/3          | 1/3         | Don asked me anything about voice there.    |
| Got it! | 1.5              | 0.9             | -0.6             | 130            | 40            | 0/3          | 0/3         | Gotted asked me anything about voice there. |

### `en-kathleen-low` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 0.0              | 2.7             | 2.7              | 0              | 0             | 0/3          | 3/3         | Oh ask me anything about voice there       |
| Hey!    | -0.0             | -0.3            | -0.3             | 10             | 0             | 1/3          | 2/3         | Hey, ask me anything about voice there.    |
| Sure!   | 1.5              | 3.2             | 1.7              | 10             | 10            | 1/3          | 3/3         | to ask me anything about voice there       |
| Yes!    | -0.5             | -2.5            | -1.9             | 10             | 10            | 1/3          | 3/3         | Yeah, ask me anything about voice there.   |
| No!     | 1.1              | 1.5             | 0.4              | 0              | 10            | 2/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 3.2              | -1.9            | -5.1             | 10             | 0             | 2/3          | 3/3         | Up ask me anything about voice there       |
| OK!     | 3.0              | 1.3             | -1.7             | 10             | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 2.4              | 1.7             | -0.8             | 10             | 0             | 0/3          | 3/3         | Gra asked me anything about voice there    |
| Thanks! | -0.3             | -1.2            | -0.9             | 10             | 20            | 2/3          | 3/3         | Thanks ask me anything about voice there   |
| Right!  | -0.6             | 1.8             | 2.4              | 0              | 0             | 1/3          | 3/3         | Rya ask me anything about voice there      |
| Hello!  | 2.8              | 2.3             | -0.5             | 0              | 0             | 2/3          | 0/3         | Hello ask me anything about voice there    |
| Well!   | 2.9              | 0.5             | -2.4             | 20             | 10            | 0/3          | 3/3         | They'll ask me anything about voice there. |
| So!     | 1.1              | 1.1             | -0.1             | 10             | 20            | 1/3          | 3/3         | Phil ask me anything about voice there     |
| Oh!     | -1.3             | -0.3            | 1.0              | 0              | 10            | 1/3          | 3/3         | Oh ask me anything about voice there       |
| Wow!    | -0.6             | 0.7             | 1.3              | 0              | 0             | 0/3          | 1/3         | Well, ask me anything about voice there    |
| Yep!    | -1.1             | -0.7            | 0.4              | 0              | 10            | 0/3          | 3/3         | ask me anything about voice there          |
| Nope!   | 0.1              | 2.1             | 2.0              | 0              | 10            | 0/3          | 3/3         | Don't ask me anything about voice there    |
| Cool!   | 3.7              | 4.3             | 0.6              | 10             | 30            | 0/3          | 3/3         | ask me anything about voice there          |
| Done!   | 1.3              | 1.9             | 0.6              | 10             | 10            | 0/3          | 3/3         | ask me anything about voice there          |
| Got it! | -1.4             | 4.2             | 5.6              | 0              | 10            | 0/3          | 2/3         | ask me anything about voice there          |

### `en-kathleen-low` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | -0.4             | 1.5             | 1.9              | 0              | 10            | 0/3          | 3/3         | Oh ask me anything about voice there      |
| Hey!    | -0.7             | -0.0            | 0.7              | 0              | 10            | 2/3          | 3/3         | Hey, ask me anything about voice there    |
| Sure!   | 0.3              | 2.8             | 2.4              | 0              | 20            | 1/3          | 3/3         | Sure ask me anything about voice there    |
| Yes!    | 0.2              | -2.7            | -2.9             | 10             | 10            | 0/3          | 3/3         | anything about voice there                |
| No!     | 0.8              | 1.1             | 0.3              | 0              | 10            | 3/3          | 3/3         | No, ask me anything about voice there     |
| Okay!   | 3.5              | -1.5            | -5.0             | 0              | 0             | 3/3          | 3/3         | Okay asked me anything about voice there. |
| OK!     | 2.8              | 1.7             | -1.1             | 0              | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | 0.8              | 1.8             | 1.1              | 0              | 0             | 0/3          | 3/3         | Ray ask me anything about voice there.    |
| Thanks! | -2.8             | -0.8            | 1.9              | 10             | 20            | 3/3          | 3/3         | Thanks. Ask me anything about voice there |
| Right!  | -1.2             | 0.5             | 1.6              | 0              | 0             | 0/3          | 3/3         | Well, ask me anything about voice there   |
| Hello!  | 2.3              | 2.0             | -0.3             | 0              | 10            | 1/3          | 2/3         | Hello ask me anything about voice there   |
| Well!   | 3.3              | 0.9             | -2.4             | 10             | 20            | 0/3          | 3/3         | Bill ask me anything about voice there    |
| So!     | -0.2             | 1.2             | 1.5              | 20             | 20            | 2/3          | 3/3         | though ask me anything about voice there  |
| Oh!     | -0.1             | -0.5            | -0.4             | 10             | 10            | 0/3          | 3/3         | ask me anything about voice there         |
| Wow!    | -0.6             | 0.8             | 1.4              | 0              | 0             | 0/3          | 2/3         | Well, ask me anything about voice there.  |
| Yep!    | -0.5             | -1.9            | -1.5             | 0              | 10            | 0/3          | 3/3         | ask me anything about voice there         |
| Nope!   | 0.9              | 2.0             | 1.2              | 0              | 10            | 0/3          | 3/3         | No, ask me anything about voice there     |
| Cool!   | 2.1              | 4.0             | 1.9              | 20             | 40            | 0/3          | 3/3         | Do ask me anything about voice there      |
| Done!   | 1.9              | 1.2             | -0.6             | 10             | 10            | 0/3          | 3/3         | Ask me anything about voice there         |
| Got it! | -0.9             | 3.8             | 4.7              | 10             | 0             | 0/3          | 2/3         | ask me anything about voice there         |

### `en-ljspeech` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)             |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------- |
| Hi!     | 0.2              | 3.4             | 3.1              | 100            | 30            | 2/3          | 2/3         | Hi, ask me anything about voice there       |
| Hey!    | -2.9             | 4.0             | 6.9              | 70             | 0             | 3/3          | 3/3         | Hey, ask me anything about voice there      |
| Sure!   | 0.5              | 1.5             | 1.1              | 60             | 60            | 3/3          | 3/3         | Sure, ask me anything about voice there     |
| Yes!    | -2.5             | 5.0             | 7.5              | 70             | 100           | 1/3          | 3/3         | Yes, ask me anything about voice there      |
| No!     | 0.9              | 4.7             | 3.8              | 70             | 70            | 1/3          | 3/3         | No, ask me anything about voice there       |
| Okay!   | 2.0              | 1.3             | -0.7             | 30             | 30            | 3/3          | 3/3         | Okay, ask me anything about voice there     |
| OK!     | 2.4              | -0.1            | -2.5             | 80             | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there     |
| Great!  | -1.1             | 3.0             | 4.0              | 90             | 100           | 2/3          | 1/3         | Great ask me anything about voice there     |
| Thanks! | -1.9             | -0.8            | 1.1              | 80             | 70            | 3/3          | 2/3         | Thanks. Ask me anything about voice there   |
| Right!  | 4.2              | 1.4             | -2.8             | 100            | 80            | 0/3          | 3/3         | Write, ask me anything about voice there    |
| Hello!  | 3.8              | 5.7             | 1.9              | 50             | 30            | 2/3          | 2/3         | Hello? asked me anything about voice there. |
| Well!   | 2.0              | 6.0             | 4.0              | 50             | 40            | 3/3          | 3/3         | Well, ask me anything about voice there.    |
| So!     | 3.6              | 3.7             | 0.1              | 40             | 30            | 3/3          | 3/3         | So ask me anything about voice there        |
| Oh!     | -0.6             | 6.6             | 7.2              | 50             | 70            | 0/3          | 1/3         | Now ask me anything about voice there       |
| Wow!    | 4.0              | 3.9             | -0.1             | 50             | 80            | 0/3          | 0/3         | Well, ask me anything about voice there     |
| Yep!    | -0.4             | 4.2             | 4.5              | 40             | 120           | 0/3          | 0/3         | Ed asked me anything about voice there.     |
| Nope!   | 1.4              | 0.8             | -0.6             | 50             | 50            | 0/3          | 0/3         | Now ask me anything about voice there       |
| Cool!   | 1.4              | 0.9             | -0.5             | 80             | 60            | 0/3          | 2/3         | Cole asked me anything about voice there    |
| Done!   | 1.1              | 0.4             | -0.7             | 80             | 40            | 1/3          | 1/3         | Don asked me anything about voice there     |
| Got it! | 1.3              | 2.9             | 1.6              | 90             | 100           | 1/3          | 0/3         | Got it asked me anything about voice there  |

### `en-ljspeech` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 0.3              | 3.3             | 3.0              | 20             | 0             | 1/3          | 2/3         | Thai asked me anything about voice there   |
| Hey!    | -2.8             | 2.5             | 5.2              | 50             | 80            | 3/3          | 3/3         | Hey, ask me anything about voice there     |
| Sure!   | 0.2              | 0.2             | 0.0              | 70             | 50            | 3/3          | 3/3         | Sure, ask me anything about voice there    |
| Yes!    | -2.7             | 4.7             | 7.4              | 60             | 70            | 2/3          | 3/3         | Pes ask me anything about voice there      |
| No!     | 2.0              | 3.1             | 1.1              | 50             | 10            | 1/3          | 3/3         | No, ask me anything about voice there      |
| Okay!   | 2.2              | 0.3             | -1.9             | 80             | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 3.0              | 0.9             | -2.1             | 60             | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 0.1              | 0.9             | 0.8              | 70             | 120           | 0/3          | 2/3         | Rate asked me anything about voice there   |
| Thanks! | -1.6             | -2.3            | -0.7             | 80             | 160           | 3/3          | 3/3         | Thanks. Ask me anything about voice there  |
| Right!  | 4.0              | 1.4             | -2.6             | 80             | 60            | 2/3          | 3/3         | Right ask me anything about voice there    |
| Hello!  | 3.4              | 4.0             | 0.6              | 30             | 40            | 2/3          | 3/3         | Allah ask me anything about voice there    |
| Well!   | 2.8              | 4.5             | 1.6              | 30             | 50            | 3/3          | 3/3         | Well, ask me anything about voice there    |
| So!     | 3.8              | 3.1             | -0.7             | 80             | 30            | 2/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | 1.7              | 3.8             | 2.1              | 30             | 30            | 0/3          | 2/3         | Well, ask me anything about Voicemair.     |
| Wow!    | 3.0              | 3.5             | 0.5              | 60             | 20            | 0/3          | 1/3         | Well, ask me anything about voice there    |
| Yep!    | 1.2              | 3.9             | 2.6              | 30             | 40            | 0/3          | 0/3         | ask me anything about voice there          |
| Nope!   | 1.0              | -0.0            | -1.0             | 20             | 70            | 1/3          | 0/3         | Now ask me anything about voice there      |
| Cool!   | 0.4              | 0.7             | 0.3              | 80             | 30            | 0/3          | 0/3         | Cole asked me anything about voice there   |
| Done!   | 2.8              | 0.1             | -2.6             | 80             | 60            | 0/3          | 0/3         | Don asked me anything abound voice there   |
| Got it! | 2.1              | 3.4             | 1.3              | 80             | 50            | 0/3          | 0/3         | but it asked me anything about voice there |

### `en-ljspeech-high` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | 2.4              | 2.3             | -0.1             | 100            | 40            | 2/3          | 2/3         | He asked me anything about voice there         |
| Hey!    | 2.1              | 1.8             | -0.3             | 90             | 70            | 2/3          | 2/3         | Hey, ask me anything about voiceair.           |
| Sure!   | 3.4              | 3.5             | 0.1              | 80             | 30            | 3/3          | 3/3         | Sure, ask me anything about voice there        |
| Yes!    | -4.7             | 0.5             | 5.2              | 110            | 80            | 2/3          | 3/3         | Yes, ask me anything about voice there         |
| No!     | -0.7             | 3.8             | 4.4              | 60             | 70            | 3/3          | 3/3         | No, ask me anything about voice there          |
| Okay!   | 0.5              | 2.3             | 1.8              | 80             | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| OK!     | 1.3              | 4.0             | 2.7              | 100            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| Great!  | -4.8             | 1.1             | 5.8              | 100            | 110           | 3/3          | 0/3         | Great. Ask me anything about voice there       |
| Thanks! | 1.3              | -1.4            | -2.7             | 90             | 40            | 3/3          | 3/3         | Thanks. Ask me anything about voice there      |
| Right!  | -1.5             | 4.0             | 5.5              | 100            | 50            | 3/3          | 2/3         | Right, ask me anything about voice there       |
| Hello!  | 4.3              | 6.0             | 1.6              | 80             | 60            | 2/3          | 1/3         | Hello ask me anything about voice there        |
| Well!   | 3.0              | 4.7             | 1.7              | 90             | 40            | 3/3          | 3/3         | Well, ask me anything about voice there        |
| So!     | 2.2              | 5.8             | 3.7              | 80             | 40            | 3/3          | 3/3         | So ask me anything about voice there           |
| Oh!     | -0.7             | 2.0             | 2.7              | 60             | 40            | 2/3          | 3/3         | Ask me anything about voice there              |
| Wow!    | -0.7             | 3.0             | 3.7              | 120            | 130           | 3/3          | 0/3         | Wow, asked me anything about voice there       |
| Yep!    | 0.8              | 1.5             | 0.8              | 110            | 10            | 0/3          | 1/3         | ask me anything about voice there.             |
| Nope!   | -0.3             | 3.0             | 3.4              | 80             | 40            | 1/3          | 2/3         | No, ask me anything about voice there          |
| Cool!   | 0.4              | 1.7             | 1.3              | 70             | 70            | 0/3          | 2/3         | Poor, ask me anything about voice there        |
| Done!   | 3.1              | -2.6            | -5.7             | 70             | 80            | 0/3          | 1/3         | Than asked me anything about voice there.      |
| Got it! | -0.5             | 3.6             | 4.1              | 110            | 70            | 0/3          | 2/3         | thought it asked me anything about voice there |

### `en-ljspeech-high` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 1.7              | 2.9             | 1.2              | 80             | 20            | 0/3          | 3/3         | Pi ask me anything about voice there.     |
| Hey!    | -0.6             | 1.0             | 1.6              | 80             | 30            | 0/3          | 3/3         | Hit ask me anything about voice there     |
| Sure!   | 2.3              | 0.9             | -1.4             | 60             | 30            | 3/3          | 3/3         | Sure ask me, anything about voice there   |
| Yes!    | -5.1             | -0.1            | 5.0              | 90             | 80            | 2/3          | 3/3         | Yes, asked me anything about voice there  |
| No!     | 0.8              | 3.2             | 2.4              | 70             | 40            | 3/3          | 3/3         | No, ask me anything about boys there.     |
| Okay!   | 0.9              | 1.9             | 1.0              | 60             | 140           | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| OK!     | 2.2              | 4.3             | 2.1              | 70             | 120           | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | -0.8             | 2.9             | 3.6              | 100            | 40            | 1/3          | 2/3         | Right, ask me anything about boys there   |
| Thanks! | -0.2             | -2.0            | -1.8             | 70             | 50            | 3/3          | 3/3         | Thanks ask me anything about voice there  |
| Right!  | -1.2             | 4.7             | 5.9              | 80             | 50            | 3/3          | 2/3         | Right, ask me anything about voice there. |
| Hello!  | 3.0              | 4.5             | 1.5              | 70             | 50            | 1/3          | 3/3         | Alo asked me anything about voice there   |
| Well!   | 3.5              | 4.4             | 0.9              | 80             | 50            | 3/3          | 3/3         | Well, ask me anything about voice there.  |
| So!     | 3.3              | 5.2             | 1.9              | 100            | 50            | 3/3          | 3/3         | So ask me, anything about voice there     |
| Oh!     | 0.2              | 5.5             | 5.2              | 70             | 80            | 2/3          | 3/3         | Oh, ask me anything about voice there     |
| Wow!    | 0.3              | 2.7             | 2.4              | 70             | 40            | 1/3          | 1/3         | Wow, ask me anything about Boyce there.   |
| Yep!    | -2.4             | 2.4             | 4.8              | 110            | 60            | 2/3          | 1/3         | Yep asked me anything about voice there   |
| Nope!   | 0.5              | 3.8             | 3.4              | 60             | 50            | 2/3          | 3/3         | Nope, ask me anything about voice there.  |
| Cool!   | 0.7              | 1.3             | 0.6              | 50             | 40            | 0/3          | 2/3         | or ask me anything about voice there.     |
| Done!   | 1.6              | 0.3             | -1.3             | 70             | 50            | 0/3          | 0/3         | Tun asked me anything about voice there.  |
| Got it! | -0.9             | 1.8             | 2.7              | 110            | 50            | 0/3          | 0/3         | That it ask me anything about voice there |

### `en-arctic` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 4.2              | 1.1             | -3.1             | 20             | 10            | 0/3          | 1/3         | I ask me anything about voice there.       |
| Hey!    | 3.3              | 0.1             | -3.2             | 20             | 10            | 2/3          | 1/3         | A, ask me anything about voice there       |
| Sure!   | 4.8              | 5.0             | 0.2              | 30             | 30            | 3/3          | 2/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.8              | 3.0             | 1.3              | 20             | 10            | 2/3          | 2/3         | Bess ask me anything about voice there     |
| No!     | 5.7              | 1.6             | -4.1             | 20             | 10            | 2/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.7              | 2.1             | -0.6             | 20             | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 2.1              | 3.8             | 1.7              | 20             | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 3.2              | 2.2             | -1.0             | 20             | 0             | 2/3          | 3/3         | Great asked me anything about voice there. |
| Thanks! | 1.7              | -2.5            | -4.2             | 10             | 10            | 2/3          | 1/3         | Banks ask me anything about voice there.   |
| Right!  | 5.5              | 1.9             | -3.6             | 40             | 20            | 2/3          | 0/3         | Right, ask me anything about voice there.  |
| Hello!  | 4.2              | 5.0             | 0.8              | 10             | 0             | 2/3          | 0/3         | Alosk me anything about voice there,       |
| Well!   | 6.3              | 2.4             | -3.9             | 30             | 10            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 7.5              | 1.4             | -6.1             | 40             | 50            | 3/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | 3.0              | 1.1             | -1.9             | 10             | 0             | 3/3          | 1/3         | Oh, ask me anything about voice there.     |
| Wow!    | 6.4              | 1.5             | -4.9             | 30             | 10            | 1/3          | 1/3         | Wow asked me anything about voice there.   |
| Yep!    | 1.8              | 1.0             | -0.8             | 20             | 20            | 1/3          | 1/3         | Depp asked me anything about voice there   |
| Nope!   | 4.1              | 1.4             | -2.7             | 20             | 20            | 3/3          | 3/3         | Nope, ask me anything about voice there    |
| Cool!   | 4.6              | 4.0             | -0.6             | 30             | 20            | 1/3          | 2/3         | Pool asked me anything about Voictail      |
| Done!   | 4.0              | 0.5             | -3.6             | 10             | 20            | 1/3          | 0/3         | Then ask me anything about voice there.    |
| Got it! | 4.8              | 2.8             | -2.0             | 30             | 20            | 1/3          | 0/3         | But it ask me anything about voice there.  |

### `en-arctic` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)              |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | -------------------------------------------- |
| Hi!     | 5.0              | 0.4             | -4.6             | 20             | 20            | 0/3          | 2/3         | I ask me anything about voiceair.            |
| Hey!    | 4.2              | 1.1             | -3.1             | 20             | 10            | 2/3          | 2/3         | Hey, ask me anything about voice there       |
| Sure!   | 4.7              | 5.4             | 0.7              | 30             | 20            | 3/3          | 2/3         | Sure, ask me anything about boys there.      |
| Yes!    | 3.8              | 3.5             | -0.3             | 20             | 0             | 2/3          | 3/3         | Yes, ask me anything about voice there.      |
| No!     | 5.1              | 1.1             | -4.0             | 20             | 20            | 3/3          | 2/3         | No, ask me anything about voice there        |
| Okay!   | 4.2              | 2.1             | -2.1             | 20             | 10            | 2/3          | 3/3         | Okay, ask me anything about Voice there      |
| OK!     | 4.6              | 4.0             | -0.6             | 20             | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there.     |
| Great!  | 4.8              | 1.6             | -3.2             | 20             | 20            | 3/3          | 2/3         | Great ask me anything about voice there      |
| Thanks! | 2.9              | -2.3            | -5.2             | 10             | 0             | 2/3          | 2/3         | Thanks. Ask me anything about voice there.   |
| Right!  | 4.5              | 3.0             | -1.5             | 30             | 10            | 2/3          | 0/3         | Write Ask me anything about voice there      |
| Hello!  | 4.9              | 4.8             | -0.1             | 30             | 10            | 2/3          | 1/3         | The law asked me anything about voice there. |
| Well!   | 5.3              | 2.8             | -2.5             | 20             | 20            | 3/3          | 3/3         | Well, ask me anything about voice there.     |
| So!     | 4.4              | 0.6             | -3.7             | 30             | 20            | 3/3          | 3/3         | So ask me anything about voice there         |
| Oh!     | 3.9              | 0.4             | -3.5             | 30             | 20            | 3/3          | 2/3         | Oh, ask me anything about voice there        |
| Wow!    | 6.0              | 2.7             | -3.3             | 20             | 10            | 1/3          | 1/3         | Well, ask me anything about voice there      |
| Yep!    | 2.3              | 0.7             | -1.6             | 30             | 20            | 0/3          | 2/3         | Ep, ask me anything about voice there        |
| Nope!   | 3.2              | 1.3             | -1.9             | 30             | 10            | 2/3          | 3/3         | Nope, ask me anything about voiceair.        |
| Cool!   | 3.0              | 3.8             | 0.8              | 10             | 20            | 2/3          | 2/3         | Pool, asked me anything about voice there    |
| Done!   | 4.9              | 0.4             | -4.6             | 10             | 10            | 1/3          | 0/3         | Don ask me anything about voice there        |
| Got it! | 4.0              | 2.9             | -1.1             | 30             | 0             | 3/3          | 1/3         | Got it ask me anything about voice there.    |

### `en-hfc-female` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 1.6              | 2.0             | 0.4              | 150            | 70            | 2/3          | 2/3         | Hey, ask me anything about voice there    |
| Hey!    | -2.1             | -1.0            | 1.1              | 70             | 60            | 3/3          | 3/3         | Hey, ask me anything about voice there.   |
| Sure!   | 0.1              | 0.9             | 0.8              | 50             | 90            | 3/3          | 3/3         | Sure, ask me anything about voice there.  |
| Yes!    | 0.4              | 0.7             | 0.3              | 80             | 80            | 3/3          | 3/3         | Yes, ask me anything about voice there.   |
| No!     | 2.4              | 1.6             | -0.8             | 30             | 30            | 3/3          | 3/3         | No, ask me anything about voice there.    |
| Okay!   | 0.6              | 0.3             | -0.2             | 80             | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| OK!     | 0.7              | -0.2            | -0.9             | 80             | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| Great!  | 0.6              | -0.3            | -0.9             | 100            | 60            | 3/3          | 3/3         | Great. Ask me anything about voice there  |
| Thanks! | 0.6              | -4.0            | -4.6             | 150            | 140           | 3/3          | 3/3         | Thanks. Ask me anything about voice there |
| Right!  | 2.4              | 2.3             | -0.1             | 120            | 60            | 3/3          | 3/3         | Right, ask me anything about voice there. |
| Hello!  | 4.5              | 2.3             | -2.2             | 120            | 90            | 3/3          | 3/3         | Hello ask me anything about voice there   |
| Well!   | 2.5              | 2.5             | -0.0             | 70             | 80            | 3/3          | 3/3         | Well, ask me anything about voice there   |
| So!     | 4.1              | 0.3             | -3.9             | 80             | 20            | 3/3          | 3/3         | So ask me anything about voice there.     |
| Oh!     | 3.6              | 3.1             | -0.5             | 130            | 30            | 3/3          | 3/3         | Oh ask me anything about voice there      |
| Wow!    | 3.5              | 2.2             | -1.4             | 60             | 30            | 2/3          | 1/3         | Wow, ask me anything about voice there.   |
| Yep!    | 1.6              | 2.9             | 1.2              | 70             | 80            | 3/3          | 3/3         | Yep, ask me anything about voice there.   |
| Nope!   | 1.2              | 0.5             | -0.7             | 80             | 50            | 3/3          | 3/3         | Nope, ask me anything about voice there.  |
| Cool!   | 0.9              | -0.2            | -1.2             | 90             | 50            | 3/3          | 2/3         | Cool. Ask me anything about voice there.  |
| Done!   | 1.3              | 0.2             | -1.1             | 50             | 80            | 1/3          | 3/3         | Done. Ask me anything about voice there.  |
| Got it! | 3.3              | 2.2             | -1.1             | 80             | 30            | 0/3          | 3/3         | Dot it. Ask me anything about voice there |

### `en-hfc-female` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 2.6              | 0.2             | -2.4             | 150            | 110           | 2/3          | 3/3         | Hey, ask me anything about voice there.    |
| Hey!    | -2.8             | -0.1            | 2.8              | 70             | 60            | 3/3          | 3/3         | Hey, ask me anything about voice there     |
| Sure!   | 0.2              | 1.6             | 1.4              | 110            | 70            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 0.2              | 0.1             | -0.2             | 70             | 80            | 3/3          | 2/3         | Yes, ask me anything about voice there.    |
| No!     | 2.1              | 2.2             | 0.1              | 50             | 30            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 1.7              | 0.2             | -1.5             | 80             | 30            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 0.5              | 0.6             | 0.2              | 80             | 30            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | -0.2             | 0.4             | 0.6              | 50             | 30            | 3/3          | 3/3         | Great. Ask me anything about voice there.  |
| Thanks! | -1.6             | -1.6            | 0.0              | 50             | 60            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 3.2              | 2.1             | -1.1             | 150            | 70            | 3/3          | 3/3         | Right. Ask me anything about voice there.  |
| Hello!  | 3.1              | 1.4             | -1.7             | 50             | 70            | 3/3          | 3/3         | Hello, ask me anything about voice there   |
| Well!   | 2.6              | 3.2             | 0.6              | 60             | 90            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 2.1              | 1.7             | -0.4             | 70             | 30            | 3/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | 1.5              | 2.0             | 0.6              | 40             | 30            | 3/3          | 3/3         | Oh, ask me anything about voice there      |
| Wow!    | 2.7              | 2.2             | -0.5             | 110            | 40            | 1/3          | 2/3         | Wow, ask me anything about voice there.    |
| Yep!    | 1.4              | 2.3             | 0.9              | 80             | 70            | 1/3          | 2/3         | Ep, ask me anything about voice there      |
| Nope!   | 1.4              | -0.5            | -1.9             | 40             | 60            | 3/3          | 3/3         | Nope. Ask me anything about voice there.   |
| Cool!   | 0.6              | -0.1            | -0.6             | 90             | 30            | 3/3          | 2/3         | Cool. Ask me anything about voice there.   |
| Done!   | 0.9              | -0.7            | -1.6             | 100            | 80            | 2/3          | 3/3         | Then ask me anything about voice there.    |
| Got it! | 3.3              | 2.0             | -1.2             | 70             | 50            | 2/3          | 2/3         | Dot it ask me anything about voice there.  |

### `en-hfc-male` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 2.4              | 3.8             | 1.3              | 60             | 30            | 3/3          | 3/3         | Hi, ask me anything about voice there.    |
| Hey!    | 0.4              | 2.5             | 2.1              | 60             | 30            | 3/3          | 3/3         | Hey, ask me anything about voice there    |
| Sure!   | -0.8             | 1.0             | 1.8              | 60             | 30            | 3/3          | 3/3         | Sure, ask me anything about voice there   |
| Yes!    | 2.2              | 1.1             | -1.1             | 90             | 50            | 3/3          | 2/3         | Yes, ask me anything about voice there.   |
| No!     | 2.6              | 1.9             | -0.7             | 40             | 70            | 3/3          | 3/3         | No, ask me anything about voice there     |
| Okay!   | 2.2              | -0.0            | -2.2             | 80             | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| OK!     | 2.5              | 0.8             | -1.7             | 80             | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | 1.2              | 0.3             | -0.9             | 150            | 50            | 3/3          | 3/3         | Great. Ask me anything about voice there  |
| Thanks! | 0.4              | 0.8             | 0.4              | 100            | 30            | 2/3          | 3/3         | Thanks ask me anything about voice there. |
| Right!  | 3.7              | 2.6             | -1.1             | 90             | 30            | 3/3          | 3/3         | Right, ask me anything about voice there. |
| Hello!  | 2.1              | 2.5             | 0.4              | 100            | 60            | 3/3          | 3/3         | Hello, ask me anything about voice there. |
| Well!   | 2.8              | 4.6             | 1.8              | 60             | 130           | 0/3          | 3/3         | Hell, ask me anything about voice there.  |
| So!     | 0.9              | 1.3             | 0.4              | 60             | 80            | 3/3          | 3/3         | So ask me anything about voice there      |
| Oh!     | -0.7             | 3.4             | 4.1              | 80             | 20            | 3/3          | 3/3         | Oh ask me anything about voice there      |
| Wow!    | 1.0              | 4.1             | 3.0              | 140            | 100           | 0/3          | 3/3         | I'll ask me anything about voice there.   |
| Yep!    | 2.4              | 2.8             | 0.3              | 80             | 40            | 2/3          | 0/3         | Yep, ask me anything about voice there    |
| Nope!   | 1.8              | 3.5             | 1.7              | 70             | 40            | 2/3          | 1/3         | Nope, ask me anything about voice there.  |
| Cool!   | -0.8             | 0.5             | 1.3              | 90             | 50            | 3/3          | 1/3         | Cool. Ask me anything about voice there.  |
| Done!   | 0.1              | 1.4             | 1.3              | 150            | 110           | 1/3          | 0/3         | Tun ask me anything about voice there.    |
| Got it! | 2.7              | 3.4             | 0.7              | 90             | 40            | 0/3          | 1/3         | At it, ask me anything about voice there. |

### `en-hfc-male` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 2.7              | 2.5             | -0.2             | 70             | 30            | 0/3          | 3/3         | Hey, ask me anything about voice there.    |
| Hey!    | 3.0              | 2.0             | -1.0             | 60             | 30            | 3/3          | 3/3         | Hey, ask me anything about voice there     |
| Sure!   | -0.7             | 1.3             | 2.0              | 70             | 50            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.7              | 2.6             | 0.8              | 70             | 30            | 3/3          | 3/3         | Yes, ask me anything about voice there     |
| No!     | 1.4              | 2.3             | 0.9              | 120            | 60            | 2/3          | 3/3         | Oh, ask me anything about voice there.     |
| Okay!   | 2.1              | 0.8             | -1.3             | 60             | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 2.0              | 0.1             | -1.9             | 110            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 1.3              | 1.5             | 0.1              | 60             | 30            | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | 0.0              | -2.0            | -2.0             | 80             | 60            | 2/3          | 3/3         | Thanks, ask me anything about voice there. |
| Right!  | 3.1              | 2.0             | -1.1             | 80             | 50            | 1/3          | 3/3         | Bite Ask me anything about voice there     |
| Hello!  | 1.7              | 2.3             | 0.6              | 70             | 40            | 3/3          | 3/3         | Hello, ask me anything about voice there   |
| Well!   | 2.5              | 4.1             | 1.6              | 100            | 120           | 2/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 1.1              | 2.1             | 1.1              | 150            | 60            | 3/3          | 3/3         | So, asked me anything about voice there.   |
| Oh!     | -0.4             | 2.4             | 2.8              | 100            | 90            | 3/3          | 3/3         | Oh ask me anything about voice there       |
| Wow!    | 1.2              | 4.5             | 3.2              | 180            | 120           | 0/3          | 2/3         | Oh, ask me anything about voice there.     |
| Yep!    | 0.9              | 3.3             | 2.4              | 80             | 30            | 0/3          | 0/3         | EP ask me anything about voice there       |
| Nope!   | 1.5              | 2.5             | 1.1              | 70             | 20            | 2/3          | 1/3         | Nope, ask me anything about voice there    |
| Cool!   | -0.8             | 0.2             | 1.0              | 100            | 20            | 3/3          | 2/3         | Cool? Ask me anything about voice there    |
| Done!   | -0.0             | 1.9             | 1.9              | 80             | 20            | 1/3          | 0/3         | Done Ask me anything about voice there.    |
| Got it! | 2.1              | 3.3             | 1.2              | 90             | 40            | 2/3          | 0/3         | Cut it. Ask me anything about voice there. |

### `en-gb-alan` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 1.9              | 1.8             | -0.2             | 90             | 30            | 2/3          | 3/3         | Hi ask me anything about voice there       |
| Hey!    | 3.1              | 1.7             | -1.4             | 100            | 30            | 2/3          | 3/3         | Hey, ask me anything about voice there     |
| Sure!   | 2.9              | 2.1             | -0.8             | 100            | 100           | 2/3          | 3/3         | Sure, ask me anything about voice there    |
| Yes!    | -1.2             | -2.0            | -0.9             | 210            | 160           | 2/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 3.8              | 0.9             | -2.9             | 100            | 150           | 2/3          | 3/3         | No, ask me anything about voice there      |
| Okay!   | 2.4              | -2.5            | -4.9             | 180            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 3.1              | 1.2             | -1.9             | 210            | 30            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 0.3              | -0.7            | -1.0             | 160            | 150           | 3/3          | 3/3         | Great. Ask me anything about voice there.  |
| Thanks! | 1.7              | -1.9            | -3.6             | 140            | 150           | 1/3          | 3/3         | Angx ask me anything about voice there.    |
| Right!  | 0.2              | -1.0            | -1.3             | 220            | 130           | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 4.0              | 2.3             | -1.7             | 210            | 160           | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 5.9              | 0.8             | -5.1             | 90             | 110           | 3/3          | 3/3         | Well, ask me anything about voice there    |
| So!     | 2.2              | 2.1             | -0.1             | 90             | 20            | 3/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | 2.6              | 2.2             | -0.4             | 90             | 30            | 3/3          | 2/3         | Oh, ask me anything about voice there      |
| Wow!    | 2.5              | 1.3             | -1.2             | 140            | 130           | 2/3          | 0/3         | Wow ask me anything about voice there      |
| Yep!    | -1.8             | 2.3             | 4.1              | 180            | 40            | 0/3          | 0/3         | Yeah, ask me anything about voice there    |
| Nope!   | 0.2              | 0.4             | 0.2              | 200            | 60            | 3/3          | 3/3         | Nope ask me anything about voice there     |
| Cool!   | 4.0              | 0.9             | -3.2             | 190            | 110           | 3/3          | 2/3         | Cool ask me anything about voice there     |
| Done!   | 1.0              | 2.4             | 1.4              | 180            | 10            | 0/3          | 2/3         | An ask me anything about voice there       |
| Got it! | 1.9              | -0.7            | -2.5             | 220            | 20            | 3/3          | 1/3         | Got it? Ask me anything about voice there. |

### `en-gb-alan` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 3.5              | -2.0            | -5.5             | 80             | 160           | 3/3          | 3/3         | Hi ask me anything about voice there      |
| Hey!    | 3.8              | -0.3            | -4.1             | 80             | 180           | 2/3          | 3/3         | Hey, ask me anything about voice there    |
| Sure!   | 3.5              | 2.2             | -1.4             | 80             | 100           | 0/3          | 3/3         | Joe asked me anything about voice there   |
| Yes!    | -2.2             | -2.9            | -0.7             | 190            | 140           | 0/3          | 3/3         | As ask me anything about voice there      |
| No!     | 5.2              | 0.5             | -4.6             | 110            | 140           | 3/3          | 3/3         | No, ask me anything about voice there     |
| Okay!   | 1.8              | -2.2            | -3.9             | 190            | 180           | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| OK!     | 2.5              | 1.0             | -1.5             | 190            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | -0.4             | -0.7            | -0.3             | 150            | 140           | 3/3          | 3/3         | Great ask me anything about voice there   |
| Thanks! | 1.7              | -2.1            | -3.9             | 160            | 120           | 1/3          | 3/3         | Thanks ask me anything about voice there  |
| Right!  | -0.1             | -0.9            | -0.9             | 200            | 130           | 3/3          | 2/3         | Right, ask me anything about voice there  |
| Hello!  | 4.5              | 2.3             | -2.2             | 210            | 120           | 3/3          | 3/3         | Hello ask me anything about voice there   |
| Well!   | 4.7              | 1.8             | -2.9             | 90             | 90            | 3/3          | 3/3         | Well, ask me anything about voice there.  |
| So!     | 2.1              | -0.9            | -3.1             | 90             | 160           | 3/3          | 3/3         | So ask me anything about voice there      |
| Oh!     | 1.9              | 1.2             | -0.7             | 80             | 150           | 3/3          | 3/3         | Oh, ask me anything about voice there     |
| Wow!    | 1.4              | 2.2             | 0.8              | 190            | 110           | 3/3          | 0/3         | Wow, ask me anything about voice there    |
| Yep!    | -1.4             | 2.0             | 3.5              | 200            | 40            | 0/3          | 0/3         | Get ask me anything about voice there     |
| Nope!   | -0.6             | 1.6             | 2.2              | 170            | 80            | 3/3          | 3/3         | Nope, ask me anything about voice there   |
| Cool!   | 3.9              | 1.2             | -2.6             | 170            | 110           | 3/3          | 0/3         | Cool ask me anything about voice there    |
| Done!   | 1.1              | 1.8             | 0.6              | 180            | 20            | 2/3          | 0/3         | An ask me anything about voice there      |
| Got it! | 1.0              | -1.8            | -2.8             | 160            | 170           | 3/3          | 0/3         | Got it? Ask me anything about voice there |

### `en-gb-cori` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | -6.4             | -0.2            | 6.1              | 170            | 40            | 0/3          | 0/3         | I ask me anything about Royce there.      |
| Hey!    | -7.2             | -1.1            | 6.1              | 70             | 50            | 3/3          | 3/3         | Hey asked me anything about voice there.  |
| Sure!   | -4.1             | -2.3            | 1.8              | 150            | 70            | 3/3          | 3/3         | Sure, ask me anything about voice there.  |
| Yes!    | -5.2             | -2.5            | 2.8              | 220            | 50            | 3/3          | 3/3         | Yes, ask me anything about voice there.   |
| No!     | -0.7             | -0.4            | 0.3              | 150            | 50            | 2/3          | 2/3         | Mo, ask me anything about voice there.    |
| Okay!   | -0.0             | -1.2            | -1.2             | 180            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| OK!     | -3.9             | -1.2            | 2.6              | 180            | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| Great!  | -4.3             | -2.3            | 2.0              | 180            | 60            | 1/3          | 3/3         | Great ask me anything about voice there.  |
| Thanks! | -5.9             | -3.8            | 2.2              | 170            | 100           | 3/3          | 3/3         | Thanks ask me anything about voice there. |
| Right!  | -4.0             | -2.0            | 2.0              | 140            | 50            | 3/3          | 3/3         | Right, ask me anything about voice there. |
| Hello!  | -1.1             | 2.4             | 3.5              | 180            | 70            | 3/3          | 3/3         | Hello ask me anything about voice there   |
| Well!   | -1.7             | 0.7             | 2.3              | 140            | 80            | 3/3          | 3/3         | Well ask me anything about voice there.   |
| So!     | -7.3             | -2.3            | 5.0              | 160            | 50            | 3/3          | 3/3         | So ask me anything about Royce there.     |
| Oh!     | -5.7             | -0.4            | 5.3              | 150            | 20            | 2/3          | 3/3         | I'll ask me anything about voice there.   |
| Wow!    | -4.3             | -1.4            | 2.8              | 180            | 10            | 2/3          | 1/3         | While ask me anything about Royce there.  |
| Yep!    | -4.8             | -4.1            | 0.7              | 90             | 80            | 1/3          | 2/3         | Yep ask me anything about voice there.    |
| Nope!   | -8.9             | -1.9            | 7.0              | 140            | 60            | 1/3          | 2/3         | Nope, ask me anything about voice there.  |
| Cool!   | -4.9             | -1.5            | 3.4              | 130            | 10            | 1/3          | 0/3         | Or ask me anything about voice there.     |
| Done!   | 0.5              | -0.9            | -1.4             | 160            | 50            | 0/3          | 3/3         | Than asked me anything about voice there. |
| Got it! | -5.2             | -3.7            | 1.5              | 210            | 70            | 0/3          | 2/3         | Otted ask me anything about voice there   |

### `en-gb-cori` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | -6.7             | 0.5             | 7.2              | 190            | 20            | 0/3          | 0/3         | I ask me anything about voice there.      |
| Hey!    | -6.5             | -3.0            | 3.5              | 200            | 70            | 3/3          | 3/3         | Hey ask me anything about voice there.    |
| Sure!   | -4.0             | -1.9            | 2.1              | 170            | 120           | 3/3          | 3/3         | Sure, ask me anything about voice there.  |
| Yes!    | -2.7             | -3.3            | -0.5             | 190            | 70            | 3/3          | 3/3         | Yes, ask me anything about voice there.   |
| No!     | -3.1             | -0.9            | 2.2              | 160            | 30            | 3/3          | 3/3         | No, ask me anything about voice there.    |
| Okay!   | -1.2             | -3.5            | -2.3             | 190            | 50            | 2/3          | 3/3         | K ask me anything about voice there.      |
| OK!     | -2.2             | -1.0            | 1.2              | 170            | 110           | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| Great!  | -3.5             | -2.9            | 0.7              | 170            | 120           | 3/3          | 2/3         | Great ask me anything about voice there.  |
| Thanks! | -4.6             | -4.0            | 0.6              | 190            | 60            | 3/3          | 3/3         | Thanks ask me anything about voice there. |
| Right!  | -4.2             | -1.0            | 3.2              | 170            | 30            | 3/3          | 3/3         | Right, ask me anything about voice there. |
| Hello!  | -0.8             | 1.1             | 1.9              | 120            | 40            | 3/3          | 3/3         | Hello ask me anything about voice there.  |
| Well!   | -2.4             | 0.6             | 3.0              | 180            | 50            | 3/3          | 3/3         | Well, ask me anything about voice there.  |
| So!     | -6.3             | -1.4            | 4.9              | 120            | 40            | 3/3          | 3/3         | So ask me anything about voice there.     |
| Oh!     | -5.4             | -0.1            | 5.3              | 160            | 80            | 3/3          | 3/3         | Oh, ask me anything about voice there     |
| Wow!    | -5.7             | -1.4            | 4.3              | 160            | 30            | 0/3          | 2/3         | While ask me anything about voice there.  |
| Yep!    | -6.3             | -3.8            | 2.5              | 130            | 130           | 1/3          | 2/3         | Ip, ask me anything about voice there.    |
| Nope!   | -7.9             | 0.9             | 8.8              | 160            | 60            | 2/3          | 0/3         | Nope, ask me anything about voice there.  |
| Cool!   | -4.8             | -2.2            | 2.6              | 110            | 60            | 1/3          | 1/3         | Gore ask me anything about voice there.   |
| Done!   | -1.1             | -2.1            | -1.1             | 180            | 100           | 0/3          | 2/3         | An ask me anything about voice there      |
| Got it! | -6.8             | -4.9            | 1.9              | 220            | 50            | 0/3          | 0/3         | otted, ask me anything about voice there. |

### `en-gb-jenny` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)             |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------- |
| Hi!     | -0.1             | -2.0            | -2.0             | 170            | 110           | 3/3          | 3/3         | Hi ask me anything about voice there.       |
| Hey!    | 0.6              | -2.2            | -2.8             | 70             | 80            | 3/3          | 3/3         | Hey, ask me anything about voice there.     |
| Sure!   | -0.3             | -1.0            | -0.7             | 110            | 90            | 3/3          | 3/3         | Sure, ask me anything about voice there.    |
| Yes!    | -0.4             | -1.1            | -0.8             | 70             | 50            | 3/3          | 3/3         | Yes, ask me anything about voice there.     |
| No!     | 0.6              | 1.1             | 0.6              | 120            | 60            | 3/3          | 3/3         | No, ask me anything about voice there.      |
| Okay!   | 0.2              | 0.3             | 0.0              | 120            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.    |
| OK!     | 1.6              | -0.3            | -1.9             | 100            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there.    |
| Great!  | -0.7             | 0.4             | 1.1              | 80             | 50            | 3/3          | 3/3         | Great ask me anything about voice there     |
| Thanks! | 1.0              | -1.7            | -2.7             | 40             | 50            | 3/3          | 3/3         | Thanks ask me anything about voice there.   |
| Right!  | 0.1              | -1.5            | -1.6             | 110            | 40            | 3/3          | 1/3         | Right, ask me anything about voice there.   |
| Hello!  | 0.5              | 0.9             | 0.4              | 110            | 50            | 3/3          | 3/3         | Hello ask me anything about voice there.    |
| Well!   | 0.6              | -0.6            | -1.2             | 100            | 70            | 3/3          | 3/3         | Well, ask me anything about voice there.    |
| So!     | 0.5              | -0.5            | -1.0             | 60             | 90            | 1/3          | 0/3         | though ask me anything about voice there.   |
| Oh!     | 0.2              | 0.2             | -0.0             | 70             | 140           | 3/3          | 3/3         | Oh, ask me anything about voice there.      |
| Wow!    | 1.0              | -0.7            | -1.7             | 50             | 70            | 3/3          | 2/3         | Wow, ask me anything about voice there      |
| Yep!    | 0.1              | 0.0             | -0.0             | 70             | 50            | 1/3          | 1/3         | Jeck, ask me anything about voice there.    |
| Nope!   | 1.8              | 0.4             | -1.5             | 60             | 30            | 2/3          | 1/3         | Nope, ask me anything about boys there.     |
| Cool!   | 1.1              | 2.5             | 1.4              | 50             | 50            | 1/3          | 3/3         | Who asked me anything about voice there.    |
| Done!   | 0.1              | -0.4            | -0.5             | 90             | 40            | 2/3          | 0/3         | Done ask me anything about voice there.     |
| Got it! | 0.4              | -0.2            | -0.6             | 80             | 60            | 2/3          | 3/3         | But it asked me anything about voice there. |

### `en-gb-jenny` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | -0.2             | -1.2            | -1.0             | 100            | 70            | 2/3          | 3/3         | I ask me anything about voice there.      |
| Hey!    | 0.3              | -1.6            | -1.9             | 100            | 50            | 3/3          | 3/3         | Hey, ask me anything about voice there    |
| Sure!   | 0.0              | -0.6            | -0.6             | 100            | 50            | 3/3          | 3/3         | Sure, ask me anything about voice there.  |
| Yes!    | 0.0              | -0.3            | -0.3             | 50             | 40            | 3/3          | 3/3         | Yes, ask me anything about voice there.   |
| No!     | 1.4              | 0.3             | -1.1             | 60             | 60            | 3/3          | 3/3         | No, ask me anything about voice there.    |
| Okay!   | 1.0              | -0.1            | -1.1             | 80             | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| OK!     | 0.4              | 1.3             | 0.9              | 120            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| Great!  | -1.9             | 0.1             | 1.9              | 130            | 60            | 3/3          | 3/3         | Great ask me anything about voice there.  |
| Thanks! | -0.8             | -0.2            | 0.6              | 90             | 40            | 3/3          | 3/3         | Thanks ask me anything about voice there. |
| Right!  | -0.4             | -1.5            | -1.1             | 80             | 50            | 3/3          | 1/3         | Right, ask me anything about voice there. |
| Hello!  | 1.1              | 1.8             | 0.8              | 150            | 50            | 3/3          | 3/3         | Hello, ask me anything about voice there. |
| Well!   | 0.1              | -1.0            | -1.1             | 50             | 40            | 3/3          | 3/3         | Well, ask me anything about voice there.  |
| So!     | 0.1              | -1.0            | -1.1             | 80             | 140           | 1/3          | 3/3         | So ask me anything about voice there.     |
| Oh!     | 0.2              | -1.6            | -1.8             | 80             | 60            | 3/3          | 3/3         | Oh, ask me anything about voice there     |
| Wow!    | 0.8              | -0.1            | -0.9             | 80             | 60            | 3/3          | 1/3         | Wow, ask me anything about voice there.   |
| Yep!    | 0.5              | -0.8            | -1.3             | 60             | 40            | 2/3          | 1/3         | Yeah, ask me anything about voice there.  |
| Nope!   | 1.2              | -0.4            | -1.6             | 110            | 40            | 3/3          | 2/3         | Nope, ask me anything about voice there.  |
| Cool!   | 1.7              | 1.3             | -0.4             | 40             | 50            | 1/3          | 2/3         | Who asked me anything about voice there?  |
| Done!   | 0.7              | -0.5            | -1.2             | 60             | 50            | 1/3          | 1/3         | John ask me anything about voice there.   |
| Got it! | 0.4              | -0.4            | -0.8             | 70             | 70            | 2/3          | 2/3         | Got it ask me anything about voice there. |

### `en-kitten-f2` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | -10.7            | -2.1            | 8.6              | 230            | 80            | 0/3          | 3/3         | Ask me anything about voice there              |
| Hey!    | -6.0             | -0.1            | 5.9              | 170            | 70            | 0/3          | 3/3         | Okay, we can ask me anything about voice there |
| Sure!   | -20.6            | 1.2             | 21.8             | 170            | 80            | 0/3          | 3/3         | Ask me anything about voice there              |
| Yes!    | -6.3             | 0.2             | 6.5              | 170            | 50            | 3/3          | 3/3         | Yes, ask me anything about voice there         |
| No!     | -8.2             | 4.5             | 12.7             | 220            | 60            | 3/3          | 3/3         | No, ask me anything about voice there          |
| Okay!   | -4.1             | 1.4             | 5.5              | 270            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| OK!     | -2.5             | 0.4             | 2.9              | 240            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| Great!  | -11.8            | 2.0             | 13.8             | 220            | 100           | 3/3          | 3/3         | Great. Ask me anything about voice there       |
| Thanks! | -3.4             | 1.4             | 4.8              | 170            | 40            | 3/3          | 3/3         | Thanks. Ask me anything about voice there      |
| Right!  | -0.4             | 0.7             | 1.1              | 170            | 90            | 3/3          | 3/3         | right. Ask me anything about voice there       |
| Hello!  | -5.2             | 4.3             | 9.5              | 180            | 80            | 3/3          | 3/3         | Hello Ask me anything about voice there        |
| Well!   | -4.3             | 2.8             | 7.2              | 230            | 60            | 3/3          | 3/3         | Well, ask me anything about voice there        |
| So!     | -8.0             | 2.2             | 10.2             | 210            | 60            | 3/3          | 3/3         | So ask me anything about voice there           |
| Oh!     | -3.3             | 2.6             | 5.9              | 170            | 40            | 3/3          | 3/3         | Oh, I can ask me anything about voice there    |
| Wow!    | -0.3             | 2.4             | 2.7              | 170            | 50            | 0/3          | 3/3         | Well, like ask me anything about voice there   |
| Yep!    | -7.4             | -0.8            | 6.6              | 160            | 110           | 0/3          | 3/3         | Ask me anything about voice there              |
| Nope!   | -1.9             | 4.6             | 6.4              | 230            | 80            | 3/3          | 3/3         | Nope. Ask me anything about voice there        |
| Cool!   | -12.1            | 1.3             | 13.3             | 170            | 40            | 3/3          | 3/3         | Cool. Ask me anything about voice there        |
| Done!   | -4.9             | 3.0             | 7.9              | 220            | 80            | 3/3          | 3/3         | Done. Ask me anything about voice there        |
| Got it! | -0.9             | 1.0             | 1.9              | 70             | 70            | 3/3          | 3/3         | Got it. Ask me anything about voice there      |

### `en-kitten-f2` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                 |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------------- |
| Hi!     | -12.2            | -2.0            | 10.2             | 220            | 50            | 0/3          | 3/3         | I ask me anything about voice there             |
| Hey!    | -6.3             | -0.6            | 5.7              | 160            | 60            | 0/3          | 3/3         | K fore ask me anything about voice there        |
| Sure!   | -21.9            | 0.7             | 22.6             | 160            | 60            | 0/3          | 3/3         | Ask me anything about voice there               |
| Yes!    | -6.4             | -0.3            | 6.1              | 160            | 50            | 3/3          | 3/3         | Yes, ask me anything about voice there          |
| No!     | -9.1             | 4.4             | 13.4             | 220            | 60            | 3/3          | 3/3         | No, ask me anything about voice there           |
| Okay!   | -5.4             | 1.0             | 6.4              | 210            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there         |
| OK!     | -3.2             | 0.3             | 3.5              | 250            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there         |
| Great!  | -10.3            | 1.4             | 11.7             | 200            | 80            | 3/3          | 3/3         | Great. Ask me anything about voice there        |
| Thanks! | -3.9             | 0.4             | 4.3              | 160            | 30            | 3/3          | 3/3         | Thanks for to ask me anything about voice there |
| Right!  | -0.8             | -0.4            | 0.4              | 160            | 80            | 2/3          | 3/3         | Right, ask me anything about voice there        |
| Hello!  | -6.2             | 4.1             | 10.3             | 160            | 70            | 3/3          | 3/3         | Hello, ask me anything about voice there        |
| Well!   | -6.8             | 2.3             | 9.2              | 210            | 50            | 3/3          | 3/3         | Well, ask me anything about voice there         |
| So!     | -10.7            | 1.2             | 11.9             | 220            | 60            | 3/3          | 3/3         | So ask me anything about voice there            |
| Oh!     | -3.2             | 2.0             | 5.3              | 160            | 40            | 3/3          | 0/3         | Oh, I can ask me anything about voice there     |
| Wow!    | -1.3             | 2.3             | 3.6              | 160            | 50            | 0/3          | 3/3         | Well, let's ask me anything about voice there   |
| Yep!    | -18.8            | -1.5            | 17.2             | 150            | 80            | 0/3          | 3/3         | I ask me anything about voice there             |
| Nope!   | -5.2             | 3.4             | 8.6              | 180            | 70            | 3/3          | 3/3         | Nope. Ask me anything about voice there         |
| Cool!   | -12.9            | 1.2             | 14.1             | 160            | 40            | 3/3          | 3/3         | Cool ask me anything about voice there          |
| Done!   | -6.4             | 2.9             | 9.3              | 220            | 70            | 3/3          | 3/3         | Done. Ask me anything about voice there         |
| Got it! | -1.4             | 1.0             | 2.5              | 150            | 50            | 3/3          | 3/3         | Got it. Ask me anything about voice there       |

### `en-kitten-m2` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                  |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------------ |
| Hi!     | -18.9            | -5.0            | 14.0             | 90             | 90            | 0/3          | 3/3         | Ask me anything about voice there                |
| Hey!    | -7.4             | -0.3            | 7.2              | 90             | 100           | 0/3          | 3/3         | Ask me anything about voice there                |
| Sure!   | -20.5            | -2.1            | 18.4             | 80             | 80            | 0/3          | 3/3         | Ask me anything about voice there                |
| Yes!    | -8.5             | -0.3            | 8.2              | 90             | 110           | 3/3          | 3/3         | Yes, but ask me anything about voice there       |
| No!     | -15.3            | -0.3            | 15.0             | 80             | 100           | 1/3          | 3/3         | Ask me anything about voice there                |
| Okay!   | -9.8             | -3.1            | 6.8              | 120            | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there          |
| OK!     | -3.9             | -1.1            | 2.8              | 110            | 80            | 1/3          | 3/3         | hit OK. Ask me anything about voice there        |
| Great!  | -12.0            | 1.6             | 13.6             | 80             | 90            | 2/3          | 3/3         | Great. Ask me anything about voice there         |
| Thanks! | -5.0             | -1.4            | 3.6              | 90             | 120           | 0/3          | 3/3         | thinks, ask me anything about voice there        |
| Right!  | -2.3             | -0.8            | 1.5              | 90             | 90            | 0/3          | 3/3         | Great God. Ask me anything about voice there     |
| Hello!  | -16.7            | -0.8            | 15.8             | 90             | 90            | 0/3          | 3/3         | Ask me anything about voice there                |
| Well!   | -7.1             | 0.4             | 7.5              | 80             | 110           | 3/3          | 3/3         | Well, ask me anything about voice there          |
| So!     | -18.0            | 0.3             | 18.3             | 90             | 90            | 0/3          | 3/3         | Ask me anything about voice there                |
| Oh!     | -2.4             | 1.2             | 3.6              | 80             | 80            | 0/3          | 3/3         | Idren, ask me anything about voice there         |
| Wow!    | -0.1             | -0.8            | -0.6             | 90             | 90            | 0/3          | 3/3         | Well, ask me anything about voice there          |
| Yep!    | -17.8            | -0.3            | 17.5             | 90             | 120           | 0/3          | 3/3         | Ask me anything about voice there                |
| Nope!   | -9.5             | -0.2            | 9.3              | 100            | 80            | 3/3          | 3/3         | Nope. Ask me anything about voice there          |
| Cool!   | -8.1             | -4.8            | 3.3              | 90             | 100           | 0/3          | 3/3         | A cool thread. Ask me anything about voice there |
| Done!   | -11.3            | -0.9            | 10.4             | 110            | 110           | 3/3          | 3/3         | Done, ask me anything about voice there          |
| Got it! | -4.2             | -1.6            | 2.6              | 120            | 60            | 3/3          | 3/3         | Got it. Ask me anything about voice there        |

### `en-kitten-m2` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                 |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------------- |
| Hi!     | -21.6            | -2.8            | 18.8             | 80             | 80            | 0/3          | 3/3         | Ask me anything about voice there.              |
| Hey!    | -7.2             | -0.1            | 7.1              | 80             | 80            | 0/3          | 3/3         | Ask me anything about voice there               |
| Sure!   | -20.4            | 0.3             | 20.7             | 80             | 80            | 0/3          | 3/3         | Ask me anything about voice there               |
| Yes!    | -7.6             | -0.2            | 7.3              | 80             | 90            | 3/3          | 3/3         | Yes, but ask me anything about voice there.     |
| No!     | -16.2            | 1.1             | 17.3             | 80             | 70            | 2/3          | 3/3         | Note what? Ask me anything about voice there    |
| Okay!   | -12.9            | -2.3            | 10.7             | 240            | 80            | 3/3          | 3/3         | Okay. Ask me anything about voice there.        |
| OK!     | -5.3             | -1.3            | 4.0              | 100            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there         |
| Great!  | -11.8            | 1.8             | 13.6             | 80             | 90            | 3/3          | 3/3         | Great. Ask me anything about voice there.       |
| Thanks! | -7.1             | -2.5            | 4.6              | 80             | 100           | 0/3          | 3/3         | Ask me anything about voice there               |
| Right!  | -2.0             | -0.4            | 1.7              | 80             | 70            | 0/3          | 3/3         | Great God. Ask me anything about voice there    |
| Hello!  | -19.8            | 0.2             | 20.0             | 80             | 100           | 0/3          | 3/3         | Ask me anything about voice there               |
| Well!   | -11.3            | 1.1             | 12.4             | 140            | 100           | 3/3          | 3/3         | Well, ask me anything about voice there         |
| So!     | -22.5            | 1.4             | 24.0             | 80             | 90            | 0/3          | 3/3         | Ask me anything about voice there               |
| Oh!     | -4.0             | 1.6             | 5.6              | 80             | 80            | 0/3          | 0/3         | Ask me anything about voice there               |
| Wow!    | -0.3             | 0.7             | 0.9              | 80             | 90            | 0/3          | 3/3         | We have one, ask me anything about voice there. |
| Yep!    | -17.6            | -1.4            | 16.2             | 80             | 100           | 0/3          | 3/3         | Ask me anything about voice there               |
| Nope!   | -9.6             | 1.1             | 10.7             | 80             | 70            | 0/3          | 0/3         | Note. Ask me anything about voice there         |
| Cool!   | -12.0            | -0.7            | 11.3             | 80             | 90            | 0/3          | 3/3         | Ask me anything about voice there               |
| Done!   | -11.1            | 0.0             | 11.1             | 100            | 80            | 0/3          | 3/3         | I'd done, ask me anything about voice there.    |
| Got it! | -6.7             | -0.7            | 6.0              | 110            | 80            | 3/3          | 3/3         | Got it. Ask me anything about voice there.      |

### `en-kitten-f3` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                  |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------------ |
| Hi!     | -10.4            | -1.0            | 9.4              | 220            | 70            | 3/3          | 3/3         | Hi, ask me anything about voice there            |
| Hey!    | -7.4             | -1.2            | 6.2              | 170            | 70            | 1/3          | 3/3         | Hey, ask me anything about voice there           |
| Sure!   | -7.3             | 0.4             | 7.8              | 180            | 70            | 0/3          | 3/3         | Ask me anything about voice there                |
| Yes!    | -8.8             | 0.5             | 9.3              | 150            | 70            | 3/3          | 3/3         | Yes, ask me anything about voice there           |
| No!     | -6.7             | 2.6             | 9.2              | 180            | 70            | 3/3          | 3/3         | No, ask me anything about voice there            |
| Okay!   | -6.8             | 0.3             | 7.1              | 200            | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there          |
| OK!     | -2.6             | 1.6             | 4.2              | 230            | 60            | 3/3          | 3/3         | Okay, ask me anything about voice there          |
| Great!  | -0.8             | -0.6            | 0.2              | 170            | 60            | 3/3          | 3/3         | Great Ask me anything about voice there          |
| Thanks! | -5.4             | 0.0             | 5.4              | 140            | 50            | 3/3          | 3/3         | Thanks. Watch ask me anything about voice there  |
| Right!  | -3.3             | 1.6             | 4.9              | 140            | 60            | 3/3          | 3/3         | Right thoughts ask me anything about voice there |
| Hello!  | -8.2             | 3.3             | 11.5             | 190            | 60            | 3/3          | 3/3         | Hello, ask me anything about voice there         |
| Well!   | -5.8             | 1.6             | 7.4              | 170            | 60            | 3/3          | 3/3         | Well, ask me anything about voice there          |
| So!     | -22.4            | 1.5             | 23.9             | 200            | 60            | 0/3          | 3/3         | Ask me anything about voice there                |
| Oh!     | -4.0             | 2.3             | 6.3              | 140            | 140           | 0/3          | 0/3         | Ask me anything about voice there                |
| Wow!    | -1.1             | 2.5             | 3.6              | 140            | 60            | 1/3          | 3/3         | Well, like ask me anything about voice there     |
| Yep!    | -19.0            | -1.2            | 17.8             | 210            | 70            | 0/3          | 3/3         | Yet ask me anything about voice there            |
| Nope!   | -5.8             | 3.3             | 9.1              | 170            | 60            | 3/3          | 3/3         | Nope. Ask me anything about voice there.         |
| Cool!   | -8.6             | 0.8             | 9.4              | 150            | 60            | 3/3          | 3/3         | Cool to like ask me anything about voice there   |
| Done!   | -4.1             | 2.0             | 6.1              | 210            | 70            | 2/3          | 3/3         | Done, ask me anything about voice there          |
| Got it! | 0.2              | 1.8             | 1.6              | 200            | 70            | 3/3          | 3/3         | Got it. Ask me anything about voice there        |

### `en-kitten-f3` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                 |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------------- |
| Hi!     | -4.7             | -1.2            | 3.5              | 190            | 60            | 0/3          | 3/3         | I ask me anything about voice there             |
| Hey!    | -6.5             | -1.6            | 5.0              | 160            | 60            | 1/3          | 3/3         | Okay, ask me anything about voice there.        |
| Sure!   | -7.7             | 0.2             | 7.9              | 130            | 60            | 0/3          | 3/3         | Ask me anything about voice there               |
| Yes!    | -10.2            | -1.8            | 8.5              | 130            | 50            | 3/3          | 3/3         | Yes, ask me anything about voice there.         |
| No!     | -3.9             | 1.7             | 5.6              | 170            | 60            | 3/3          | 3/3         | No, ask me anything about voice there.          |
| Okay!   | -6.6             | 0.3             | 6.9              | 180            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there         |
| OK!     | -3.0             | 1.2             | 4.2              | 220            | 60            | 2/3          | 3/3         | Okay. Ask me anything about voice there         |
| Great!  | -15.1            | -1.0            | 14.1             | 150            | 50            | 3/3          | 3/3         | Great. Ask me anything about voice there.       |
| Thanks! | -5.7             | -2.0            | 3.7              | 130            | 40            | 3/3          | 3/3         | Thanks. Ask me anything about voice there       |
| Right!  | -3.6             | 0.5             | 4.1              | 130            | 50            | 3/3          | 3/3         | Right thought ask me anything about voice there |
| Hello!  | -6.6             | 2.7             | 9.3              | 170            | 50            | 1/3          | 3/3         | Hello, ask me anything about voice there.       |
| Well!   | -5.6             | 1.4             | 7.0              | 180            | 50            | 3/3          | 3/3         | Well, ask me anything about voice there.        |
| So!     | -6.4             | 1.0             | 7.4              | 170            | 60            | 0/3          | 3/3         | Ask me anything about voice there               |
| Oh!     | -4.4             | 2.8             | 7.1              | 130            | 120           | 0/3          | 3/3         | So I ask me anything about voice there          |
| Wow!    | -1.2             | 2.4             | 3.6              | 130            | 50            | 0/3          | 3/3         | Well, ask me anything about voice there         |
| Yep!    | -0.1             | -0.1            | -0.0             | 130            | 60            | 0/3          | 3/3         | Ask me anything about voice there               |
| Nope!   | 0.0              | 2.7             | 2.7              | 130            | 50            | 3/3          | 3/3         | Nope, ask me anything about voice there         |
| Cool!   | -7.2             | 0.5             | 7.7              | 130            | 60            | 0/3          | 3/3         | Ask me anything about voice there               |
| Done!   | -6.0             | 1.0             | 7.0              | 190            | 50            | 3/3          | 3/3         | Done, ask me anything about voice there.        |
| Got it! | -0.6             | 2.1             | 2.7              | 180            | 60            | 3/3          | 3/3         | Got it. Ask me anything about voice there       |

### `en-kitten-m3` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | -13.1            | -2.2            | 10.9             | 90             | 90            | 0/3          | 3/3         | like ask me anything about voice there         |
| Hey!    | -7.6             | -1.2            | 6.4              | 90             | 80            | 3/3          | 3/3         | Hey, but ask me anything about voice there.    |
| Sure!   | -18.5            | -1.3            | 17.2             | 100            | 90            | 0/3          | 3/3         | Ask me anything about voice there              |
| Yes!    | -5.2             | -0.6            | 4.6              | 90             | 90            | 3/3          | 3/3         | Yes, but ask me anything about voice there.    |
| No!     | -5.3             | 0.9             | 6.2              | 100            | 100           | 1/3          | 3/3         | Note what? Ask me anything about voice there.  |
| Okay!   | -7.1             | -3.4            | 3.7              | 100            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| OK!     | -4.0             | -1.3            | 2.7              | 110            | 80            | 3/3          | 3/3         | OK. Ask me anything about voice there.         |
| Great!  | -10.4            | -0.4            | 10.0             | 100            | 90            | 3/3          | 3/3         | Great like ask me anything about voice there   |
| Thanks! | -3.3             | 0.4             | 3.6              | 90             | 140           | 3/3          | 3/3         | Thanks, but ask me anything about voice there  |
| Right!  | -2.4             | 0.4             | 2.9              | 90             | 50            | 3/3          | 0/3         | Right, like Ask me anything about voice there. |
| Hello!  | -16.5            | -1.1            | 15.4             | 90             | 130           | 0/3          | 3/3         | He looked, ask me anything about voice there   |
| Well!   | -9.7             | 0.3             | 10.0             | 100            | 70            | 3/3          | 3/3         | Well, ask me anything about voice there        |
| So!     | -13.0            | -0.6            | 12.5             | 90             | 100           | 3/3          | 3/3         | So it like ask me anything about voice there   |
| Oh!     | -2.2             | 0.5             | 2.7              | 100            | 80            | 0/3          | 3/3         | Ask me anything about voice there              |
| Wow!    | -1.5             | -0.1            | 1.3              | 100            | 80            | 0/3          | 3/3         | Well, what? Ask me anything about voice there  |
| Yep!    | -11.5            | -0.7            | 10.9             | 90             | 70            | 0/3          | 3/3         | It ask me anything about voice there.          |
| Nope!   | -7.5             | 1.9             | 9.3              | 100            | 50            | 2/3          | 3/3         | Nope. Ask me anything about voice there.       |
| Cool!   | -5.9             | 0.4             | 6.3              | 100            | 90            | 0/3          | 3/3         | Gootheg. Ask me anything about voice there.    |
| Done!   | -9.7             | -0.0            | 9.6              | 130            | 110           | 0/3          | 3/3         | I've done. Ask me anything about voice there.  |
| Got it! | 0.2              | 0.1             | -0.0             | 130            | 100           | 3/3          | 3/3         | Got it. Ask me anything about voice there      |

### `en-kitten-m3` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                   |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------------- |
| Hi!     | -12.6            | -1.4            | 11.2             | 80             | 70            | 0/3          | 0/3         | Ask me anything about voice there                 |
| Hey!    | -7.6             | -0.8            | 6.8              | 80             | 90            | 2/3          | 3/3         | Hey, but ask me anything about voice there.       |
| Sure!   | -18.5            | 0.1             | 18.6             | 90             | 60            | 0/3          | 3/3         | Ask me anything about voice there.                |
| Yes!    | -4.9             | -0.6            | 4.3              | 90             | 70            | 3/3          | 3/3         | Yes, but ask me anything about voice there.       |
| No!     | -6.7             | 1.1             | 7.8              | 80             | 90            | 2/3          | 3/3         | No plot. Ask me anything about voice there.       |
| Okay!   | -12.3            | -1.5            | 10.7             | 90             | 120           | 1/3          | 3/3         | The Cape. Ask me anything about voice there.      |
| OK!     | -4.5             | -1.2            | 3.3              | 160            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there.          |
| Great!  | -11.2            | -0.4            | 10.8             | 80             | 80            | 2/3          | 3/3         | Great. Ask me anything about voice there.         |
| Thanks! | -3.6             | 0.4             | 4.0              | 80             | 130           | 0/3          | 3/3         | thinks, like ask me anything about voice there    |
| Right!  | -2.4             | 0.4             | 2.8              | 80             | 50            | 2/3          | 2/3         | But I thought. Ask me anything about voice there. |
| Hello!  | -17.2            | 1.1             | 18.4             | 90             | 120           | 0/3          | 3/3         | Ask me anything about voice there.                |
| Well!   | -11.6            | 1.1             | 12.7             | 90             | 100           | 3/3          | 3/3         | Well, ask me anything about voice there.          |
| So!     | -13.1            | 0.6             | 13.7             | 80             | 80            | 3/3          | 3/3         | So ask me anything about voice there              |
| Oh!     | -2.3             | 0.5             | 2.7              | 90             | 80            | 0/3          | 3/3         | Ask me anything about voice there                 |
| Wow!    | -1.7             | 0.5             | 2.2              | 90             | 60            | 0/3          | 3/3         | Well, what? Ask me anything about voice there.    |
| Yep!    | -11.5            | -1.8            | 9.7              | 80             | 70            | 0/3          | 3/3         | It ask me anything about voice there              |
| Nope!   | -8.0             | 2.0             | 10.0             | 80             | 50            | 3/3          | 3/3         | Nope. Ask me anything about voice there.          |
| Cool!   | -6.9             | 0.9             | 7.8              | 80             | 90            | 0/3          | 3/3         | Ask me anything about voice there                 |
| Done!   | -14.7            | 0.5             | 15.2             | 270            | 90            | 2/3          | 3/3         | I've done. Ask me anything about voice there.     |
| Got it! | -1.9             | 0.5             | 2.4              | 130            | 100           | 3/3          | 3/3         | Got it. Ask me anything about voice there.        |

### `en-kitten-f4` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                  |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------------ |
| Hi!     | -7.9             | -2.9            | 5.0              | 190            | 60            | 0/3          | 3/3         | Ask me anything about voice there                |
| Hey!    | -8.3             | -2.8            | 5.5              | 150            | 70            | 3/3          | 3/3         | Hey for it ask me anything about voice there     |
| Sure!   | -13.9            | -1.3            | 12.6             | 140            | 60            | 0/3          | 3/3         | Ask me anything about voice there                |
| Yes!    | -10.7            | -1.5            | 9.2              | 140            | 60            | 3/3          | 3/3         | Yes, what ask me anything about voice there      |
| No!     | -7.4             | 3.2             | 10.7             | 190            | 60            | 3/3          | 3/3         | No, ask me anything about voice there            |
| Okay!   | -7.3             | -0.4            | 6.9              | 200            | 20            | 3/3          | 3/3         | Okay, ask me anything about voice there          |
| OK!     | -4.7             | -0.8            | 3.8              | 220            | 90            | 3/3          | 3/3         | Okay. Ask me anything about voice there          |
| Great!  | -4.3             | -1.5            | 2.8              | 170            | 70            | 3/3          | 3/3         | Great ask me anything about voice there          |
| Thanks! | -5.0             | 2.5             | 7.6              | 140            | 140           | 3/3          | 3/3         | Thanks, like ask me anything about voice there   |
| Right!  | -3.3             | -2.5            | 0.9              | 140            | 80            | 3/3          | 3/3         | right thought, ask me anything about voice there |
| Hello!  | -10.4            | 1.2             | 11.6             | 180            | 30            | 3/3          | 3/3         | Hello, ask me anything about voice there         |
| Well!   | -8.8             | -1.3            | 7.5              | 190            | 70            | 3/3          | 3/3         | Well, ask me anything about voice there          |
| So!     | -17.0            | -1.7            | 15.3             | 160            | 70            | 3/3          | 3/3         | So ask me anything about voice there             |
| Oh!     | -5.3             | -0.3            | 5.0              | 140            | 50            | 0/3          | 3/3         | You might ask me anything about voice there.     |
| Wow!    | -3.4             | -0.3            | 3.1              | 140            | 60            | 0/3          | 3/3         | Well, though, ask me anything about voice there  |
| Yep!    | -13.3            | -2.9            | 10.4             | 140            | 80            | 0/3          | 3/3         | Ask me anything about voice there                |
| Nope!   | -6.0             | 3.2             | 9.2              | 170            | 90            | 3/3          | 3/3         | Nope. Ask me anything about voice there.         |
| Cool!   | -15.7            | -1.4            | 14.4             | 140            | 70            | 0/3          | 3/3         | I ask me anything about voice there              |
| Done!   | -7.9             | 1.1             | 9.0              | 200            | 60            | 3/3          | 3/3         | Done, ask me anything about voice there          |
| Got it! | -1.4             | -0.1            | 1.2              | 170            | 40            | 3/3          | 3/3         | Got it. Ask me anything about voice there        |

### `en-kitten-f4` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                  |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------------ |
| Hi!     | -5.1             | -3.5            | 1.6              | 160            | 50            | 0/3          | 3/3         | I ask me anything about voice there              |
| Hey!    | -8.5             | -2.6            | 5.9              | 130            | 50            | 0/3          | 3/3         | taith ask me anything about voice there          |
| Sure!   | -6.2             | -1.7            | 4.4              | 120            | 50            | 0/3          | 3/3         | Ask me anything about voice there.               |
| Yes!    | -10.7            | -3.8            | 6.9              | 130            | 50            | 3/3          | 3/3         | Yes, what ask me anything about voice there      |
| No!     | -5.8             | 1.9             | 7.7              | 160            | 50            | 3/3          | 3/3         | No, ask me anything about voice there.           |
| Okay!   | -8.1             | -0.1            | 8.0              | 170            | 140           | 3/3          | 3/3         | Okay, ask me anything about voice there.         |
| OK!     | -5.5             | -1.0            | 4.6              | 190            | 80            | 3/3          | 3/3         | Okay. Ask me anything about voice there          |
| Great!  | -18.3            | -1.4            | 16.9             | 140            | 60            | 3/3          | 3/3         | Great. Ask me anything about voice there.        |
| Thanks! | -6.1             | 1.6             | 7.7              | 130            | 130           | 3/3          | 3/3         | Thanks. Why ask me anything about voice there    |
| Right!  | -4.2             | -3.4            | 0.9              | 120            | 60            | 3/3          | 3/3         | Right thought ask me anything about voice there. |
| Hello!  | -11.0            | 1.7             | 12.7             | 170            | 140           | 3/3          | 3/3         | Hello, ask me anything about voice there.        |
| Well!   | -6.6             | -0.8            | 5.8              | 170            | 50            | 3/3          | 3/3         | Well, ask me anything about voice there.         |
| So!     | -24.1            | -2.0            | 22.1             | 140            | 50            | 0/3          | 3/3         | Ask me anything about voice there                |
| Oh!     | -5.1             | -0.5            | 4.6              | 130            | 40            | 0/3          | 3/3         | You might ask me anything about voice there      |
| Wow!    | -2.6             | -1.2            | 1.4              | 130            | 40            | 0/3          | 3/3         | Well, try to ask me anything about voice there   |
| Yep!    | -5.0             | -1.9            | 3.2              | 130            | 80            | 0/3          | 3/3         | Ask me anything about voice there                |
| Nope!   | -3.0             | 1.9             | 4.9              | 170            | 70            | 0/3          | 3/3         | No, ask me anything about voice there.           |
| Cool!   | -15.7            | -1.8            | 13.9             | 140            | 50            | 0/3          | 3/3         | Well, if I ask me anything about voice there     |
| Done!   | -7.7             | -0.0            | 7.7              | 170            | 50            | 3/3          | 3/3         | Done. Ask me anything about voice there.         |
| Got it! | -2.1             | 0.0             | 2.1              | 160            | 150           | 3/3          | 3/3         | Got it. Ask me anything about voice there        |

### `en-kitten-m4` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)               |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | --------------------------------------------- |
| Hi!     | -15.8            | -1.1            | 14.6             | 100            | 70            | 0/3          | 1/3         | Ask me anything about voice there             |
| Hey!    | -7.6             | -1.2            | 6.4              | 100            | 90            | 0/3          | 3/3         | Ask me anything about voice there             |
| Sure!   | -22.5            | -3.9            | 18.6             | 100            | 90            | 0/3          | 3/3         | Ask me anything about voice there             |
| Yes!    | -12.2            | -0.2            | 12.0             | 100            | 80            | 0/3          | 3/3         | Ask me anything about voice there             |
| No!     | -10.2            | 0.5             | 10.7             | 100            | 80            | 0/3          | 3/3         | Nope. Ask me anything about voice there       |
| Okay!   | -10.9            | 0.0             | 10.9             | 100            | 90            | 3/3          | 3/3         | Okay, like ask me anything about voice there  |
| OK!     | -3.0             | 0.4             | 3.5              | 130            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there       |
| Great!  | -7.4             | 0.4             | 7.8              | 100            | 70            | 0/3          | 3/3         | Ask me anything about voice there             |
| Thanks! | -2.8             | -0.5            | 2.3              | 100            | 140           | 0/3          | 3/3         | thinks what Ask me anything about voice there |
| Right!  | -2.8             | 0.9             | 3.8              | 100            | 70            | 3/3          | 0/3         | Right one. Ask me anything about voice there  |
| Hello!  | -14.9            | 1.6             | 16.6             | 100            | 100           | 0/3          | 3/3         | believing. Ask me anything about voice there  |
| Well!   | -9.1             | 1.1             | 10.2             | 100            | 90            | 3/3          | 3/3         | Well, ask me anything about voice there       |
| So!     | -15.7            | -0.2            | 15.5             | 100            | 80            | 0/3          | 3/3         | Ask me anything about voice there             |
| Oh!     | -6.3             | 2.6             | 8.9              | 100            | 100           | 0/3          | 0/3         | Ask me anything about voice there             |
| Wow!    | -3.4             | 0.6             | 4.0              | 100            | 80            | 0/3          | 2/3         | Ask me anything about voice there             |
| Yep!    | -16.6            | -1.2            | 15.4             | 100            | 80            | 0/3          | 3/3         | Ask me anything about voice there             |
| Nope!   | -10.3            | 2.4             | 12.6             | 110            | 70            | 3/3          | 3/3         | Nope. Ask me anything about voice there,      |
| Cool!   | -12.2            | -2.4            | 9.9              | 100            | 100           | 3/3          | 3/3         | cool thing. Ask me anything about voice there |
| Done!   | -18.5            | -0.2            | 18.3             | 100            | 70            | 0/3          | 3/3         | Ask me anything about voice there             |
| Got it! | -3.8             | 0.5             | 4.2              | 110            | 70            | 3/3          | 0/3         | Got it. Ask me anything about voice there     |

### `en-kitten-m4` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)              |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | -------------------------------------------- |
| Hi!     | -18.1            | -0.8            | 17.3             | 90             | 80            | 0/3          | 0/3         | Ask me anything about voice there            |
| Hey!    | -10.2            | -1.1            | 9.1              | 90             | 70            | 0/3          | 3/3         | Ask me anything about voice there            |
| Sure!   | -23.6            | -0.8            | 22.9             | 90             | 80            | 0/3          | 3/3         | Ask me anything about voice there            |
| Yes!    | -10.6            | -0.3            | 10.3             | 100            | 70            | 0/3          | 3/3         | Ask me anything about voice there.           |
| No!     | -10.8            | 1.7             | 12.5             | 90             | 70            | 0/3          | 3/3         | Ask me anything about voice there            |
| Okay!   | -11.2            | 0.6             | 11.7             | 100            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there      |
| OK!     | -3.8             | -0.1            | 3.6              | 120            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there      |
| Great!  | -10.0            | 0.3             | 10.3             | 100            | 70            | 0/3          | 3/3         | Read. Ask me anything about voice there      |
| Thanks! | -4.0             | -1.1            | 2.8              | 90             | 140           | 2/3          | 3/3         | Thinks. Ask me anything about voice there    |
| Right!  | -3.0             | 0.8             | 3.8              | 90             | 60            | 0/3          | 0/3         | Write one. Ask me anything about voice there |
| Hello!  | -16.1            | 2.7             | 18.8             | 90             | 110           | 0/3          | 3/3         | Ask me anything about voice there            |
| Well!   | -10.7            | 1.8             | 12.5             | 90             | 70            | 0/3          | 3/3         | We'll ask me anything about voice there      |
| So!     | -15.8            | -0.0            | 15.8             | 90             | 80            | 0/3          | 3/3         | Ask me anything about voice there            |
| Oh!     | -6.4             | 2.9             | 9.3              | 100            | 100           | 0/3          | 0/3         | Ask me anything about voice there            |
| Wow!    | -5.3             | 2.0             | 7.3              | 100            | 60            | 0/3          | 0/3         | Ask me anything about voice there.           |
| Yep!    | -16.6            | -1.2            | 15.4             | 100            | 80            | 0/3          | 3/3         | Ask me anything about voice there            |
| Nope!   | -11.1            | 2.2             | 13.3             | 100            | 70            | 3/3          | 3/3         | Nope, ask me anything about voice there.     |
| Cool!   | -12.4            | -1.4            | 11.0             | 90             | 100           | 0/3          | 3/3         | Ask me anything about voice there.           |
| Done!   | -22.3            | 0.3             | 22.6             | 100            | 70            | 0/3          | 3/3         | Ask me anything about voice there.           |
| Got it! | -4.5             | 1.3             | 5.8              | 110            | 70            | 3/3          | 0/3         | Got it. Ask me anything about voice there    |

### `en-kitten-f5` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | -5.9             | 0.3             | 6.2              | 160            | 100           | 0/3          | 3/3         | type Ask me anything about voice there         |
| Hey!    | -8.5             | -0.8            | 7.8              | 160            | 100           | 0/3          | 3/3         | Ask me anything about voice there              |
| Sure!   | -3.5             | 2.8             | 6.3              | 160            | 100           | 0/3          | 3/3         | Ask me anything about voice there              |
| Yes!    | -5.9             | -0.1            | 5.8              | 160            | 110           | 3/3          | 3/3         | Yes, but ask me anything about voice there.    |
| No!     | -4.9             | 2.8             | 7.7              | 160            | 100           | 0/3          | 3/3         | Notebook. Ask me anything about voice there.   |
| Okay!   | -8.2             | 0.9             | 9.1              | 170            | 100           | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| OK!     | -0.3             | 2.5             | 2.9              | 260            | 100           | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| Great!  | -5.9             | -0.6            | 5.4              | 160            | 110           | 3/3          | 3/3         | Great that ask me anything about voice there   |
| Thanks! | -3.0             | -2.7            | 0.4              | 160            | 100           | 0/3          | 3/3         | thinks once ask me anything about voice there  |
| Right!  | -4.6             | 0.5             | 5.1              | 160            | 50            | 0/3          | 3/3         | Great one. Ask me anything about voice there   |
| Hello!  | -9.7             | 3.4             | 13.1             | 160            | 100           | 0/3          | 3/3         | Look. Ask me anything about voice there        |
| Well!   | -5.1             | 2.5             | 7.6              | 160            | 100           | 3/3          | 3/3         | Well, ask me anything about voice there        |
| So!     | -3.9             | 1.4             | 5.3              | 160            | 90            | 3/3          | 3/3         | So ask me anything about voice there           |
| Oh!     | -2.8             | 2.8             | 5.5              | 160            | 100           | 0/3          | 3/3         | Ask me anything about voice there.             |
| Wow!    | -2.4             | 2.2             | 4.6              | 160            | 100           | 0/3          | 0/3         | Well, do I ask me anything about voice there,  |
| Yep!    | -2.7             | 0.0             | 2.8              | 160            | 100           | 0/3          | 3/3         | Ask me anything about voice there              |
| Nope!   | -8.8             | 2.8             | 11.6             | 160            | 60            | 3/3          | 3/3         | Nope ask me anything about voice there         |
| Cool!   | -3.6             | 2.6             | 6.3              | 160            | 50            | 3/3          | 3/3         | Cool things. Ask me anything about voice there |
| Done!   | -10.0            | 0.9             | 10.9             | 160            | 100           | 3/3          | 3/3         | Done. Ask me anything about voice there.       |
| Got it! | -0.5             | 2.2             | 2.7              | 210            | 110           | 3/3          | 3/3         | Got it. Ask me anything about voice there      |

### `en-kitten-f5` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | -3.6             | 1.1             | 4.7              | 140            | 70            | 0/3          | 3/3         | Ask me anything about voice there              |
| Hey!    | -6.2             | 0.9             | 7.1              | 140            | 80            | 0/3          | 3/3         | Ask me anything about voice there.             |
| Sure!   | -3.1             | 2.8             | 5.9              | 140            | 80            | 0/3          | 3/3         | Ask me anything about voice there.             |
| Yes!    | -5.1             | -0.1            | 5.0              | 140            | 90            | 3/3          | 3/3         | Yes, ask me anything about voice there.        |
| No!     | -4.3             | 3.0             | 7.2              | 160            | 80            | 1/3          | 3/3         | Note ask me anything about voice there.        |
| Okay!   | -13.4            | 0.7             | 14.1             | 140            | 100           | 3/3          | 3/3         | Okay, ask me anything about voice there.       |
| OK!     | -0.3             | 2.7             | 3.1              | 220            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there.       |
| Great!  | -5.1             | 0.2             | 5.3              | 140            | 100           | 3/3          | 3/3         | Great, but ask me anything about voice there   |
| Thanks! | -3.1             | -0.5            | 2.6              | 140            | 60            | 1/3          | 3/3         | Thinks once ask me anything about voice there. |
| Right!  | -4.6             | 0.1             | 4.8              | 140            | 50            | 1/3          | 3/3         | Right. Ask me anything about voice there.      |
| Hello!  | -6.3             | 3.1             | 9.4              | 140            | 60            | 0/3          | 3/3         | Ask me anything about voice there              |
| Well!   | -6.4             | 3.1             | 9.5              | 140            | 80            | 3/3          | 3/3         | Well, ask me anything about voice there        |
| So!     | -2.2             | 1.7             | 3.8              | 140            | 90            | 3/3          | 3/3         | So ask me anything about voice there           |
| Oh!     | -2.7             | 2.2             | 5.0              | 140            | 90            | 0/3          | 3/3         | Ask me anything about voice there.             |
| Wow!    | -3.7             | 2.5             | 6.2              | 140            | 80            | 0/3          | 0/3         | Well, ask me anything about voice there        |
| Yep!    | -2.6             | 0.3             | 3.0              | 140            | 100           | 0/3          | 3/3         | Ask me anything about voice there              |
| Nope!   | -3.6             | 2.3             | 5.9              | 140            | 60            | 0/3          | 3/3         | Ask me anything about voice there.             |
| Cool!   | -2.0             | 2.4             | 4.4              | 140            | 40            | 0/3          | 3/3         | Cooling. Ask me anything about voice there.    |
| Done!   | -11.4            | 2.1             | 13.5             | 140            | 80            | 2/3          | 3/3         | Ask me anything about voice there.             |
| Got it! | -1.8             | 2.9             | 4.6              | 190            | 80            | 3/3          | 3/3         | Got it. Ask me anything about voice there.     |

### `en-kitten-m5` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                  |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------------ |
| Hi!     | -9.9             | -1.4            | 8.5              | 100            | 120           | 0/3          | 3/3         | Ask me anything about voice there                |
| Hey!    | -3.1             | 1.0             | 4.1              | 100            | 130           | 0/3          | 3/3         | Ask me anything about voice there                |
| Sure!   | -12.4            | -3.0            | 9.5              | 100            | 110           | 0/3          | 3/3         | to ask me anything about voice there             |
| Yes!    | -3.2             | -0.2            | 3.0              | 100            | 120           | 3/3          | 3/3         | Yes, but ask me anything about voice there       |
| No!     | -1.7             | 0.5             | 2.2              | 100            | 110           | 2/3          | 3/3         | Not. Ask me anything about voice there           |
| Okay!   | -3.9             | -0.7            | 3.2              | 130            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there          |
| OK!     | 1.4              | -0.6            | -1.9             | 160            | 100           | 3/3          | 3/3         | Okay, ask me anything about voice there          |
| Great!  | -9.3             | -1.5            | 7.8              | 130            | 80            | 3/3          | 3/3         | Great like ask me anything about voice there     |
| Thanks! | -3.9             | 0.3             | 4.2              | 90             | 60            | 3/3          | 3/3         | Thanks. What ask me anything about voice there   |
| Right!  | -0.4             | 0.6             | 0.9              | 100            | 70            | 2/3          | 3/3         | Right thought, ask me anything about voice there |
| Hello!  | -8.9             | -0.0            | 8.9              | 120            | 60            | 3/3          | 3/3         | Hello. Ask me anything about voice there         |
| Well!   | -1.8             | -0.4            | 1.3              | 110            | 130           | 3/3          | 3/3         | Well, ask me anything about voice there          |
| So!     | -7.2             | -1.5            | 5.6              | 100            | 120           | 3/3          | 3/3         | So ask me anything about voice there             |
| Oh!     | -3.0             | 1.5             | 4.6              | 100            | 120           | 0/3          | 3/3         | Ask me anything about voice there                |
| Wow!    | -0.7             | -0.4            | 0.3              | 100            | 120           | 0/3          | 3/3         | We have ask me anything about voice there        |
| Yep!    | -8.2             | -0.8            | 7.4              | 100            | 150           | 3/3          | 3/3         | Yep. Ask me anything about voice there           |
| Nope!   | -4.4             | 2.1             | 6.5              | 100            | 60            | 3/3          | 3/3         | Nope, ask me anything about voice there          |
| Cool!   | -3.2             | -0.6            | 2.6              | 100            | 110           | 3/3          | 3/3         | Cool, fine. Ask me anything about voice there    |
| Done!   | -5.6             | -0.5            | 5.1              | 120            | 110           | 0/3          | 3/3         | being done, ask me anything about voice there    |
| Got it! | -0.1             | -0.8            | -0.7             | 160            | 60            | 3/3          | 3/3         | Got it ask me anything about voice there         |

### `en-kitten-m5` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)                |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ---------------------------------------------- |
| Hi!     | -10.2            | -1.3            | 8.9              | 100            | 100           | 0/3          | 3/3         | I ask me anything about voice there.           |
| Hey!    | -4.2             | 0.7             | 4.9              | 100            | 110           | 2/3          | 3/3         | Tay from ask me anything about voice there     |
| Sure!   | -12.6            | -1.5            | 11.1             | 100            | 100           | 0/3          | 3/3         | to ask me anything about voice there           |
| Yes!    | -3.1             | -0.2            | 2.9              | 100            | 120           | 3/3          | 3/3         | Yes, but ask me anything about voice there     |
| No!     | -2.5             | 0.5             | 3.0              | 90             | 120           | 0/3          | 3/3         | Notepot. Ask me anything about voice there     |
| Okay!   | -4.6             | -1.4            | 3.3              | 120            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| OK!     | -0.1             | -1.5            | -1.4             | 140            | 100           | 3/3          | 3/3         | Okay, ask me anything about voice there        |
| Great!  | -11.7            | 0.1             | 11.8             | 120            | 90            | 3/3          | 3/3         | Great like Ask me anything about voice there   |
| Thanks! | -4.2             | -0.7            | 3.6              | 100            | 40            | 2/3          | 3/3         | Thanks, but ask me anything about voice there  |
| Right!  | -0.5             | -0.3            | 0.2              | 90             | 70            | 3/3          | 3/3         | Right for Ask me anything about voice there    |
| Hello!  | -11.1            | 0.8             | 12.0             | 120            | 80            | 3/3          | 3/3         | Hello. Ask me anything about voice there       |
| Well!   | -4.5             | 0.8             | 5.3              | 110            | 120           | 3/3          | 3/3         | Well, I ask me anything about voice there      |
| So!     | -6.8             | -0.0            | 6.8              | 100            | 110           | 3/3          | 3/3         | So ask me anything about voice there           |
| Oh!     | -4.2             | 1.6             | 5.8              | 100            | 110           | 0/3          | 3/3         | Ask me anything about voice there              |
| Wow!    | -0.6             | 0.2             | 0.8              | 100            | 110           | 0/3          | 3/3         | We have fun. Ask me anything about voice there |
| Yep!    | -11.1            | -1.4            | 9.8              | 100            | 140           | 0/3          | 3/3         | Ask me anything about voice there.             |
| Nope!   | -6.5             | 1.2             | 7.7              | 90             | 50            | 1/3          | 3/3         | No, ask me anything about voice there.         |
| Cool!   | -3.9             | 0.9             | 4.9              | 90             | 50            | 3/3          | 3/3         | Cool fine. Ask me anything about voice there   |
| Done!   | -9.3             | 0.4             | 9.8              | 120            | 100           | 3/3          | 3/3         | Done. Ask me anything about voice there        |
| Got it! | -0.6             | 0.4             | 1.0              | 130            | 100           | 3/3          | 3/3         | Got it. Ask me anything about voice there      |

### `en-kokoro-heart` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 1.1              | -0.1            | -1.3             | 110            | 140           | 3/3          | 3/3         | Hi, ask me anything about voice there      |
| Hey!    | 1.4              | 0.3             | -1.1             | 90             | 130           | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 1.5              | -0.1            | -1.6             | 120            | 80            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 2.2              | 0.1             | -2.1             | 140            | 100           | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 2.2              | 3.0             | 0.8              | 100            | 130           | 0/3          | 3/3         | Now ask me anything about voice there      |
| Okay!   | 2.8              | 1.1             | -1.7             | 130            | 150           | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 2.8              | 1.5             | -1.2             | 130            | 130           | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 2.6              | 0.2             | -2.3             | 130            | 100           | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | 2.1              | -0.5            | -2.6             | 130            | 120           | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 2.2              | -0.3            | -2.5             | 130            | 120           | 3/3          | 2/3         | Right, ask me anything about voice there   |
| Hello!  | 2.4              | 3.0             | 0.6              | 130            | 150           | 3/3          | 3/3         | Hello Ask me anything about voice there    |
| Well!   | 1.7              | 1.6             | -0.1             | 120            | 100           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 1.4              | 0.5             | -1.0             | 120            | 90            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 1.5              | 1.9             | 0.4              | 100            | 120           | 3/3          | 3/3         | Oh, ask me anything about voice there      |
| Wow!    | 1.7              | 0.5             | -1.1             | 140            | 120           | 3/3          | 3/3         | Wow, ask me anything about voice there     |
| Yep!    | 2.0              | 0.3             | -1.7             | 120            | 90            | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 2.9              | 2.6             | -0.3             | 120            | 90            | 3/3          | 3/3         | Nope, ask me anything about voice there    |
| Cool!   | 2.5              | 1.7             | -0.8             | 140            | 170           | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | 1.6              | 1.5             | -0.1             | 130            | 130           | 3/3          | 3/3         | Done, ask me anything about voice there.   |
| Got it! | 2.3              | 0.9             | -1.4             | 120            | 10            | 3/3          | 0/3         | Got it. Ask me anything about voice there. |

### `en-kokoro-nicole` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 3.6              | 1.2             | -2.4             | 150            | 90            | 0/3          | 0/3         | I ask me anything about voice there       |
| Hey!    | 3.8              | 1.9             | -2.0             | 130            | 140           | 0/3          | 3/3         | He ask me anything about voice there.     |
| Sure!   | 3.6              | 1.7             | -1.9             | 140            | 110           | 3/3          | 3/3         | Sure, ask me anything about voice there.  |
| Yes!    | 4.0              | -2.3            | -6.2             | 140            | 90            | 3/3          | 3/3         | Yes, ask me anything about voice there.   |
| No!     | 3.4              | 2.3             | -1.2             | 100            | 90            | 3/3          | 3/3         | No, ask me anything about voice there     |
| Okay!   | 4.0              | -2.0            | -6.0             | 160            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there.  |
| OK!     | 4.1              | -2.4            | -6.5             | 150            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | 3.8              | -0.7            | -4.5             | 130            | 90            | 3/3          | 3/3         | great ask me anything about voice there   |
| Thanks! | 3.9              | -1.0            | -4.9             | 150            | 60            | 0/3          | 3/3         | Ask me anything about voice there.        |
| Right!  | 3.3              | 0.3             | -3.1             | 130            | 70            | 3/3          | 3/3         | right ask me anything about voice there   |
| Hello!  | 3.4              | 0.0             | -3.4             | 160            | 40            | 3/3          | 3/3         | Hello ask me anything about voice there   |
| Well!   | 3.3              | 0.9             | -2.4             | 130            | 40            | 3/3          | 3/3         | Well, ask me anything about voice there   |
| So!     | 3.7              | 1.3             | -2.4             | 150            | 40            | 3/3          | 3/3         | So ask me anything about voice there.     |
| Oh!     | 3.8              | 1.9             | -1.9             | 130            | 40            | 3/3          | 2/3         | Oh, ask me anything about voice there     |
| Wow!    | 3.2              | 1.1             | -2.0             | 120            | 150           | 0/3          | 0/3         | Well, ask me anything about voice there.  |
| Yep!    | 3.2              | 0.9             | -2.3             | 120            | 120           | 3/3          | 3/3         | Yep, ask me anything about voice there.   |
| Nope!   | 3.6              | 0.4             | -3.2             | 140            | 90            | 3/3          | 3/3         | Nope, ask me anything about voice there.  |
| Cool!   | 3.9              | 1.2             | -2.6             | 150            | 70            | 3/3          | 3/3         | cool ask me anything about voice there.   |
| Done!   | 2.9              | 1.2             | -1.7             | 140            | 120           | 3/3          | 3/3         | Done, ask me anything about voice there   |
| Got it! | 3.2              | -4.3            | -7.6             | 160            | 80            | 3/3          | 3/3         | got it ask me anything about voice there. |

### `en-kokoro-nicole` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 3.3              | 0.7             | -2.6             | 140            | 80            | 3/3          | 0/3         | Hi ask me anything about voice there       |
| Hey!    | 3.0              | 0.7             | -2.2             | 120            | 150           | 0/3          | 3/3         | Her he asked me anything about voice there |
| Sure!   | 3.1              | 2.0             | -1.1             | 130            | 110           | 3/3          | 3/3         | Sure, ask me anything about voice there    |
| Yes!    | 3.6              | -0.9            | -4.5             | 120            | 120           | 3/3          | 3/3         | Yes, ask me anything about voice there     |
| No!     | 3.1              | 1.0             | -2.2             | 100            | 80            | 3/3          | 3/3         | No, ask me anything about voice there      |
| Okay!   | 3.3              | -3.6            | -7.0             | 140            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 3.4              | -4.8            | -8.1             | 140            | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 3.4              | -1.6            | -5.0             | 130            | 90            | 3/3          | 3/3         | great ask me anything about voice there    |
| Thanks! | 3.5              | -2.7            | -6.2             | 140            | 80            | 3/3          | 3/3         | Thanks ask me anything about voice there   |
| Right!  | 2.9              | -0.3            | -3.2             | 130            | 120           | 3/3          | 3/3         | right ask me anything about voice there    |
| Hello!  | 2.9              | -0.9            | -3.8             | 130            | 60            | 3/3          | 3/3         | Hello, ask me anything about voice there   |
| Well!   | 2.8              | 0.2             | -2.6             | 120            | 170           | 3/3          | 3/3         | Well, ask me anything about voice there    |
| So!     | 3.1              | -2.9            | -6.0             | 120            | 160           | 3/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | 3.3              | 0.3             | -3.0             | 120            | 170           | 3/3          | 3/3         | Oh ask me anything about voice there       |
| Wow!    | 2.5              | 0.4             | -2.1             | 120            | 170           | 0/3          | 0/3         | Well, ask me anything about voice there    |
| Yep!    | 2.9              | -8.5            | -11.4            | 120            | 170           | 3/3          | 3/3         | Yep ask me anything about voice there      |
| Nope!   | 3.4              | 0.6             | -2.8             | 130            | 80            | 3/3          | 3/3         | Nope ask me anything about voice there     |
| Cool!   | 3.7              | 0.6             | -3.1             | 150            | 60            | 3/3          | 3/3         | Cool ask me anything about voice there     |
| Done!   | 2.2              | 0.2             | -2.0             | 130            | 120           | 3/3          | 3/3         | Done ask me anything about voice there     |
| Got it! | 2.6              | -6.4            | -9.1             | 140            | 70            | 2/3          | 3/3         | Got it ask me anything about voice there   |

### `en-kokoro-sarah` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 1.5              | 1.3             | -0.3             | 160            | 60            | 3/3          | 3/3         | Hi Ask me anything about voice there.      |
| Hey!    | 1.6              | 2.5             | 0.9              | 170            | 60            | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 2.5              | 1.6             | -0.9             | 180            | 60            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 2.5              | 0.9             | -1.6             | 190            | 80            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 1.9              | 2.6             | 0.7              | 170            | 90            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 3.0              | 2.8             | -0.2             | 190            | 120           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 3.0              | 3.4             | 0.4              | 180            | 130           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 3.1              | 1.6             | -1.6             | 180            | 90            | 3/3          | 3/3         | Great. Ask me anything about voice there.  |
| Thanks! | 2.0              | -0.5            | -2.5             | 180            | 100           | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 2.2              | 1.7             | -0.5             | 160            | 90            | 3/3          | 0/3         | Right, ask me anything about voice there.  |
| Hello!  | 2.6              | 2.9             | 0.3              | 190            | 90            | 3/3          | 3/3         | Hello Ask me anything about voice there    |
| Well!   | 1.8              | 2.0             | 0.2              | 170            | 50            | 0/3          | 3/3         | Wells, ask me anything about voice there.  |
| So!     | 1.8              | 1.8             | 0.0              | 180            | 40            | 3/3          | 3/3         | So, ask me anything about voice there.     |
| Oh!     | 1.9              | 3.0             | 1.1              | 170            | 40            | 3/3          | 0/3         | Oh, ask me anything about voice there.     |
| Wow!    | 1.7              | 2.1             | 0.5              | 170            | 90            | 0/3          | 0/3         | Well, ask me anything about voice there.   |
| Yep!    | 1.9              | 1.7             | -0.2             | 170            | 60            | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 2.5              | 2.0             | -0.5             | 170            | 90            | 3/3          | 3/3         | Nope. Ask me anything about voice there.   |
| Cool!   | 2.9              | 1.6             | -1.3             | 190            | 100           | 3/3          | 3/3         | Cool. Ask me anything about voice there.   |
| Done!   | 1.8              | 2.1             | 0.3              | 180            | 90            | 0/3          | 3/3         | Duns, ask me anything about voice there.   |
| Got it! | 2.9              | 1.3             | -1.6             | 180            | 160           | 3/3          | 3/3         | Got it. Ask me anything about voice there. |

### `en-kokoro-michael` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)           |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ----------------------------------------- |
| Hi!     | 2.4              | 2.2             | -0.3             | 130            | 90            | 3/3          | 3/3         | Hi, ask me anything about voice there     |
| Hey!    | 3.5              | 2.2             | -1.3             | 130            | 100           | 1/3          | 3/3         | Hello, ask me anything about voice there  |
| Sure!   | 3.4              | 1.1             | -2.3             | 110            | 80            | 3/3          | 3/3         | Sure, ask me anything about voice there   |
| Yes!    | 4.5              | 3.1             | -1.4             | 140            | 100           | 3/3          | 3/3         | Yes, ask me anything about voice there    |
| No!     | 2.9              | 3.8             | 0.9              | 130            | 100           | 3/3          | 3/3         | No, ask me anything about voice there     |
| Okay!   | 4.2              | 3.2             | -1.0             | 140            | 70            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| OK!     | 4.3              | 3.7             | -0.6             | 140            | 90            | 3/3          | 3/3         | Okay, ask me anything about voice there   |
| Great!  | 4.5              | 4.1             | -0.5             | 130            | 170           | 3/3          | 3/3         | Great Ask me anything about voice there   |
| Thanks! | 4.3              | 0.8             | -3.4             | 150            | 70            | 3/3          | 3/3         | Thanks. Ask me anything about voice there |
| Right!  | 3.9              | 2.7             | -1.2             | 130            | 110           | 3/3          | 0/3         | Right, ask me anything about voice there  |
| Hello!  | 3.6              | 6.1             | 2.5              | 160            | 100           | 3/3          | 3/3         | Hello Ask me anything about voice there   |
| Well!   | 3.3              | 4.3             | 1.0              | 120            | 70            | 3/3          | 3/3         | Well, ask me anything about voice there   |
| So!     | 3.7              | 2.6             | -1.1             | 120            | 70            | 3/3          | 3/3         | So, ask me anything about voice there     |
| Oh!     | 3.0              | 4.5             | 1.5              | 120            | 80            | 3/3          | 1/3         | Oh ask me anything about voice there      |
| Wow!    | 2.9              | 2.3             | -0.6             | 140            | 100           | 3/3          | 3/3         | Wow, ask me anything about voice there    |
| Yep!    | 4.0              | 3.3             | -0.7             | 140            | 90            | 3/3          | 3/3         | Yep, ask me anything about voice there    |
| Nope!   | 4.0              | 4.5             | 0.5              | 130            | 100           | 3/3          | 3/3         | Nope, ask me anything about voice there   |
| Cool!   | 3.1              | 1.4             | -1.8             | 140            | 110           | 3/3          | 3/3         | Cool. Ask me anything about voice there   |
| Done!   | 3.6              | 3.5             | -0.2             | 150            | 110           | 3/3          | 3/3         | Done, ask me anything about voice there   |
| Got it! | 3.7              | 4.3             | 0.6              | 120            | 100           | 3/3          | 0/3         | Got it. Ask me anything about voice there |

### `en-kokoro-michael` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 2.0              | 2.4             | 0.4              | 110            | 70            | 3/3          | 3/3         | Hi, ask me anything about voice there.     |
| Hey!    | 2.6              | 3.6             | 1.0              | 120            | 60            | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 2.8              | 2.0             | -0.8             | 110            | 50            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 4.2              | 3.0             | -1.2             | 130            | 70            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 2.3              | 3.5             | 1.2              | 120            | 70            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 3.6              | 1.6             | -2.0             | 130            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 3.5              | 2.6             | -0.9             | 120            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 4.2              | 3.3             | -0.9             | 140            | 90            | 3/3          | 3/3         | Great Ask me anything about voice there.   |
| Thanks! | 3.4              | 1.4             | -2.1             | 120            | 70            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 3.4              | 4.2             | 0.8              | 120            | 60            | 3/3          | 0/3         | Right, ask me anything about voice there   |
| Hello!  | 3.0              | 5.7             | 2.6              | 130            | 90            | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 2.5              | 4.5             | 2.1              | 100            | 30            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 3.3              | 3.9             | 0.7              | 100            | 40            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 2.5              | 4.3             | 1.8              | 90             | 80            | 3/3          | 3/3         | Oh, ask me anything about voice there.     |
| Wow!    | 2.1              | 3.4             | 1.3              | 120            | 60            | 3/3          | 3/3         | Wow, ask me anything about voice there.    |
| Yep!    | 3.7              | 4.2             | 0.6              | 120            | 50            | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 3.6              | 4.6             | 1.0              | 110            | 60            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 2.6              | 0.5             | -2.2             | 140            | 100           | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | 3.0              | 3.3             | 0.3              | 120            | 100           | 3/3          | 3/3         | Done, ask me anything about voice there.   |
| Got it! | 3.3              | 3.7             | 0.4              | 120            | 90            | 1/3          | 3/3         | Got it. Ask me anything about voice there  |

### `en-kokoro-fenrir` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 0.3              | 0.2             | -0.1             | 90             | 70            | 3/3          | 3/3         | Hi, ask me anything about voice there.     |
| Hey!    | 0.7              | 0.9             | 0.2              | 90             | 40            | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 2.0              | 2.8             | 0.8              | 100            | 50            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.9              | -0.3            | -2.3             | 110            | 50            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 0.8              | 2.6             | 1.9              | 90             | 60            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.6              | 2.1             | -0.5             | 120            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 2.7              | 2.0             | -0.6             | 110            | 50            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 2.4              | 0.5             | -1.9             | 100            | 50            | 0/3          | 3/3         | Grades Ask me anything about voice there   |
| Thanks! | 0.9              | -0.6            | -1.6             | 100            | 50            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 1.6              | -0.1            | -1.7             | 100            | 60            | 3/3          | 3/3         | Right. Ask me anything about voice there.  |
| Hello!  | 1.8              | 3.4             | 1.7              | 90             | 70            | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 0.4              | 0.4             | -0.0             | 90             | 20            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 1.1              | 1.6             | 0.5              | 90             | 10            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 0.8              | 1.6             | 0.8              | 100            | 40            | 3/3          | 3/3         | Oh, ask me anything about voice there      |
| Wow!    | 0.3              | 0.6             | 0.3              | 110            | 50            | 3/3          | 0/3         | Wow, ask me anything about voice there.    |
| Yep!    | 1.6              | -0.3            | -1.9             | 110            | 40            | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 1.5              | 1.7             | 0.2              | 110            | 60            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 2.0              | 2.1             | 0.1              | 100            | 70            | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | 0.8              | 1.9             | 1.1              | 100            | 70            | 3/3          | 3/3         | Done, ask me anything about voice there.   |
| Got it! | 2.0              | 1.3             | -0.7             | 110            | 50            | 3/3          | 0/3         | Got it. Ask me anything about voice there. |

### `en-kokoro-fenrir` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | -0.2             | -0.1            | 0.2              | 100            | 40            | 3/3          | 3/3         | Hi, ask me anything about voice there.     |
| Hey!    | -0.1             | 0.3             | 0.4              | 110            | 30            | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 1.0              | 1.7             | 0.7              | 90             | 30            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.1              | -0.7            | -1.8             | 110            | 30            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 0.1              | 2.3             | 2.1              | 110            | 30            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.1              | 1.0             | -1.1             | 110            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 2.0              | 1.0             | -1.0             | 100            | 40            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 1.2              | 0.9             | -0.3             | 90             | 50            | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | 0.3              | -0.7            | -1.0             | 100            | 30            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 0.7              | -0.9            | -1.6             | 90             | 40            | 3/3          | 3/3         | Right. Ask me anything about voice there.  |
| Hello!  | 1.4              | 3.4             | 2.0              | 90             | 40            | 3/3          | 2/3         | Hello Ask me anything about voice there    |
| Well!   | -0.3             | 0.0             | 0.3              | 90             | 20            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 0.7              | 1.2             | 0.5              | 110            | 20            | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 0.1              | -0.1            | -0.2             | 110            | 20            | 3/3          | 3/3         | Oh, ask me anything about voice there.     |
| Wow!    | -0.3             | -0.4            | -0.1             | 90             | 30            | 0/3          | 0/3         | Well, ask me anything about voice there.   |
| Yep!    | 0.9              | -1.5            | -2.4             | 100            | 40            | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 0.9              | 1.4             | 0.5              | 110            | 40            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 1.5              | 2.7             | 1.1              | 100            | 50            | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | 0.2              | 1.3             | 1.1              | 90             | 50            | 3/3          | 3/3         | Done, ask me anything about voice there.   |
| Got it! | 1.4              | 1.1             | -0.3             | 90             | 60            | 3/3          | 0/3         | Got it. Ask me anything about voice there  |

### `en-kokoro-puck` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 1.6              | 3.3             | 1.7              | 100            | 0             | 3/3          | 3/3         | Hi, ask me anything about voice there      |
| Hey!    | 2.3              | 3.7             | 1.4              | 100            | 0             | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 2.5              | 4.5             | 2.0              | 100            | 0             | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 2.4              | 1.9             | -0.6             | 80             | 0             | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 2.0              | 5.6             | 3.5              | 80             | 0             | 0/3          | 3/3         | Now, ask me anything about voice there.    |
| Okay!   | 3.3              | 4.4             | 1.1              | 60             | 0             | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 3.4              | 5.5             | 2.1              | 60             | 0             | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 3.1              | 4.0             | 0.9              | 80             | 0             | 3/3          | 3/3         | Great, ask me anything about voice there.  |
| Thanks! | 2.1              | 2.1             | 0.0              | 100            | 0             | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 2.3              | 2.5             | 0.2              | 90             | 0             | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 3.1              | 6.5             | 3.4              | 40             | 10            | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 1.4              | 2.9             | 1.4              | 90             | 0             | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 2.7              | 3.7             | 1.0              | 110            | 0             | 3/3          | 3/3         | So ask me anything about voice there       |
| Oh!     | 2.1              | 2.6             | 0.5              | 110            | 0             | 3/3          | 3/3         | Oh, ask me anything about voice there      |
| Wow!    | 1.3              | 2.9             | 1.6              | 70             | 0             | 0/3          | 0/3         | Well, ask me anything about voice there.   |
| Yep!    | 2.2              | 1.3             | -0.9             | 100            | 0             | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 2.7              | 4.2             | 1.5              | 90             | 0             | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 3.3              | 5.1             | 1.8              | 50             | 10            | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | 2.4              | 3.8             | 1.5              | 70             | 0             | 3/3          | 3/3         | Done, ask me anything about voice there.   |
| Got it! | 3.2              | 4.6             | 1.4              | 80             | 0             | 3/3          | 0/3         | Got it. Ask me anything about voice there. |

### `en-kokoro-puck` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 0.8              | 3.1             | 2.3              | 100            | 0             | 0/3          | 0/3         | I ask me anything about voice there        |
| Hey!    | 1.2              | 3.0             | 1.8              | 90             | 0             | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 2.0              | 3.6             | 1.5              | 100            | 0             | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.9              | 1.8             | -0.1             | 100            | 0             | 3/3          | 3/3         | Yes, ask me anything about voice there     |
| No!     | 1.6              | 5.1             | 3.4              | 100            | 0             | 0/3          | 3/3         | Now, ask me anything about voice there     |
| Okay!   | 2.9              | 4.2             | 1.4              | 60             | 0             | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 3.0              | 4.5             | 1.4              | 50             | 0             | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 2.2              | 3.6             | 1.4              | 90             | 0             | 3/3          | 3/3         | Great, ask me anything about voice there.  |
| Thanks! | 1.4              | 1.7             | 0.3              | 100            | 0             | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 1.5              | 2.0             | 0.4              | 100            | 0             | 3/3          | 3/3         | Right, ask me anything about voice there.  |
| Hello!  | 2.2              | 6.1             | 3.9              | 80             | 10            | 3/3          | 3/3         | Hello ask me anything about voice there    |
| Well!   | 0.2              | 2.6             | 2.4              | 100            | 0             | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 2.0              | 2.5             | 0.5              | 110            | 0             | 3/3          | 3/3         | So ask me anything about voice there.      |
| Oh!     | 1.0              | 2.1             | 1.1              | 100            | 0             | 3/3          | 3/3         | Oh, ask me anything about voice there      |
| Wow!    | 0.8              | 2.2             | 1.4              | 100            | 0             | 0/3          | 0/3         | Well, ask me anything about voice there.   |
| Yep!    | 1.5              | 0.9             | -0.6             | 90             | 0             | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 2.2              | 3.7             | 1.5              | 100            | 0             | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 2.5              | 5.5             | 3.0              | 70             | 10            | 0/3          | 0/3         | Google, ask me anything about voice there. |
| Done!   | 1.5              | 3.1             | 1.6              | 100            | 0             | 3/3          | 3/3         | Done, ask me anything about voice there.   |
| Got it! | 2.6              | 4.3             | 1.6              | 80             | 0             | 3/3          | 3/3         | Got it. Ask me anything about voice there. |

### `en-gb-kokoro-emma` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 1.0              | 2.7             | 1.7              | 80             | 10            | 3/3          | 3/3         | Hi, ask me anything about voice there.     |
| Hey!    | 1.8              | 3.1             | 1.3              | 160            | 10            | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 0.8              | 3.9             | 3.1              | 80             | 10            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.7              | 2.2             | 0.5              | 150            | 10            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 1.7              | 4.7             | 3.0              | 90             | 20            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.8              | 3.5             | 0.7              | 110            | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| OK!     | 2.6              | 3.4             | 0.8              | 110            | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there    |
| Great!  | 2.5              | 2.4             | -0.2             | 150            | 30            | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | 1.0              | 1.3             | 0.3              | 110            | 0             | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 1.8              | 2.3             | 0.5              | 110            | 0             | 3/3          | 0/3         | Right, ask me anything about voice there   |
| Hello!  | 2.7              | 4.7             | 2.0              | 90             | 10            | 3/3          | 3/3         | Hello, ask me anything about voice there   |
| Well!   | 1.1              | 3.6             | 2.5              | 150            | 10            | 3/3          | 3/3         | Well, ask me anything about voice there    |
| So!     | 0.8              | 2.6             | 1.8              | 70             | 10            | 3/3          | 3/3         | So, ask me anything about voice there      |
| Oh!     | 1.1              | 3.6             | 2.5              | 80             | 10            | 1/3          | 0/3         | Lo ask me anything about voice there.      |
| Wow!    | 1.1              | 3.4             | 2.3              | 90             | 10            | 0/3          | 2/3         | Well, ask me anything about voice there.   |
| Yep!    | 0.9              | 1.8             | 0.9              | 120            | 40            | 3/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 1.8              | 3.3             | 1.5              | 110            | 30            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 2.3              | 3.2             | 1.0              | 140            | 40            | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | 1.4              | 3.6             | 2.2              | 140            | 10            | 3/3          | 3/3         | Done. Ask me anything about voice there.   |
| Got it! | 2.3              | 1.7             | -0.6             | 80             | 10            | 3/3          | 3/3         | Got it? Ask me anything about voice there  |

### `en-gb-kokoro-emma` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 0.7              | 2.5             | 1.9              | 150            | 10            | 3/3          | 3/3         | Hi, ask me anything about voice there.     |
| Hey!    | 1.3              | 3.0             | 1.7              | 150            | 10            | 3/3          | 3/3         | Hey, ask me anything about voice there.    |
| Sure!   | 0.8              | 3.2             | 2.5              | 130            | 10            | 3/3          | 3/3         | Sure, ask me anything about voice there.   |
| Yes!    | 1.0              | 1.8             | 0.7              | 130            | 20            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 1.2              | 4.1             | 3.0              | 110            | 10            | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.6              | 2.4             | -0.2             | 90             | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 2.8              | 3.0             | 0.3              | 100            | 10            | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 2.3              | 2.8             | 0.5              | 110            | 20            | 3/3          | 3/3         | Great. Ask me anything about voice there.  |
| Thanks! | -0.1             | 1.3             | 1.4              | 60             | 10            | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 1.0              | 1.9             | 0.9              | 110            | 0             | 3/3          | 0/3         | Right, ask me anything about voice there.  |
| Hello!  | 2.4              | 4.7             | 2.4              | 130            | 20            | 3/3          | 3/3         | Hello, ask me anything about voice there.  |
| Well!   | 0.6              | 3.2             | 2.6              | 130            | 10            | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 0.4              | 2.3             | 1.9              | 110            | 10            | 3/3          | 3/3         | So, ask me anything about voice there.     |
| Oh!     | 1.4              | 3.3             | 1.9              | 160            | 10            | 3/3          | 3/3         | Oh ask me anything about voice there.      |
| Wow!    | 1.1              | 2.8             | 1.7              | 150            | 10            | 0/3          | 0/3         | Well, ask me anything about voice there.   |
| Yep!    | 0.3              | 1.2             | 1.0              | 110            | 20            | 3/3          | 3/3         | Yep ask me anything about voice there.     |
| Nope!   | 1.5              | 2.8             | 1.2              | 100            | 30            | 3/3          | 3/3         | Nope, ask me anything about voice there.   |
| Cool!   | 1.7              | 3.8             | 2.0              | 130            | 10            | 3/3          | 0/3         | Cool. Ask me anything about voice there.   |
| Done!   | 0.8              | 3.2             | 2.5              | 100            | 10            | 3/3          | 3/3         | Done. Ask me anything about voice there.   |
| Got it! | 1.8              | 1.6             | -0.2             | 80             | 10            | 3/3          | 0/3         | Got it. Ask me anything about voice there. |

### `en-gb-kokoro-george` speed 1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)            |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------ |
| Hi!     | 2.7              | 0.4             | -2.3             | 100            | 90            | 3/3          | 3/3         | Hi, ask me anything about voice there      |
| Hey!    | 2.8              | 1.3             | -1.5             | 90             | 80            | 0/3          | 3/3         | Her ask me anything about voice there      |
| Sure!   | 2.5              | 0.9             | -1.6             | 80             | 100           | 2/3          | 3/3         | Shura, ask me anything about voice there.  |
| Yes!    | 2.7              | 0.1             | -2.6             | 80             | 90            | 3/3          | 3/3         | Yes, ask me anything about voice there.    |
| No!     | 3.0              | 1.6             | -1.5             | 90             | 100           | 3/3          | 3/3         | No, ask me anything about voice there.     |
| Okay!   | 2.6              | 2.8             | 0.1              | 110            | 130           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| OK!     | 2.7              | 2.7             | 0.0              | 100            | 120           | 3/3          | 3/3         | Okay, ask me anything about voice there.   |
| Great!  | 2.7              | 0.5             | -2.2             | 90             | 90            | 3/3          | 3/3         | Great. Ask me anything about voice there   |
| Thanks! | 2.2              | 0.3             | -1.9             | 90             | 100           | 3/3          | 3/3         | Thanks. Ask me anything about voice there. |
| Right!  | 2.6              | 0.6             | -2.0             | 90             | 80            | 2/3          | 0/3         | Ride Ask me anything about voice there     |
| Hello!  | 2.8              | 3.2             | 0.4              | 100            | 140           | 3/3          | 3/3         | Hello Ask me anything about voice there    |
| Well!   | 2.8              | 1.6             | -1.2             | 80             | 100           | 3/3          | 3/3         | Well, ask me anything about voice there.   |
| So!     | 2.5              | 1.4             | -1.1             | 100            | 110           | 3/3          | 3/3         | So, ask me anything about voice there.     |
| Oh!     | 2.7              | 1.1             | -1.6             | 90             | 90            | 3/3          | 0/3         | Oh ask me anything about voice there       |
| Wow!    | 2.8              | 1.0             | -1.8             | 100            | 90            | 0/3          | 0/3         | Well, ask me anything about voice there.   |
| Yep!    | 2.4              | 0.7             | -1.8             | 90             | 80            | 2/3          | 3/3         | Yep, ask me anything about voice there.    |
| Nope!   | 2.8              | 1.2             | -1.5             | 100            | 80            | 0/3          | 3/3         | Nobe, ask me anything about voice there.   |
| Cool!   | 3.0              | 2.3             | -0.7             | 100            | 130           | 3/3          | 3/3         | Cool, ask me anything about voice there.   |
| Done!   | 2.6              | 0.6             | -2.0             | 90             | 90            | 3/3          | 3/3         | Done. Ask me anything about voice there.   |
| Got it! | 2.9              | 0.8             | -2.1             | 90             | 120           | 0/3          | 0/3         | Guarded, ask me anything about voice there |

### `en-gb-kokoro-george` speed 1.1

| opener  | delta_db written | delta_db merged | merged − written | gap_ms written | gap_ms merged | hits written | hits merged | written heard (first synthesis)             |
| ------- | ---------------- | --------------- | ---------------- | -------------- | ------------- | ------------ | ----------- | ------------------------------------------- |
| Hi!     | 2.3              | 0.9             | -1.4             | 80             | 50            | 3/3          | 0/3         | Hi, ask me anything about voice there       |
| Hey!    | 2.6              | 1.4             | -1.2             | 90             | 80            | 0/3          | 3/3         | Harry, ask me anything about voice there    |
| Sure!   | 2.0              | 1.5             | -0.5             | 80             | 50            | 3/3          | 3/3         | Sure, ask me anything about voice there     |
| Yes!    | 2.5              | 0.1             | -2.3             | 80             | 80            | 3/3          | 3/3         | Yes, ask me anything about voice there.     |
| No!     | 2.7              | 1.4             | -1.3             | 90             | 90            | 3/3          | 3/3         | No, ask me anything about voice there       |
| Okay!   | 2.7              | 2.2             | -0.4             | 90             | 110           | 3/3          | 3/3         | Okay, ask me anything about voice there     |
| OK!     | 2.7              | 2.0             | -0.7             | 90             | 80            | 3/3          | 3/3         | Okay, ask me anything about voice there     |
| Great!  | 2.2              | 0.5             | -1.7             | 80             | 110           | 3/3          | 3/3         | Great. Ask me anything about voice there    |
| Thanks! | 1.5              | 0.3             | -1.1             | 70             | 80            | 3/3          | 3/3         | Thanks. Ask me anything about voice there   |
| Right!  | 2.3              | 0.6             | -1.7             | 80             | 90            | 2/3          | 0/3         | Right, ask me anything about voice there    |
| Hello!  | 2.7              | 2.6             | -0.2             | 90             | 100           | 3/3          | 3/3         | Hello Ask me anything about voice there     |
| Well!   | 2.0              | 1.1             | -1.0             | 90             | 80            | 3/3          | 3/3         | Well, ask me anything about voice there     |
| So!     | 2.2              | 1.1             | -1.0             | 90             | 60            | 3/3          | 3/3         | So ask me anything about voice there        |
| Oh!     | 2.1              | 0.8             | -1.3             | 70             | 40            | 3/3          | 2/3         | Oh ask me anything about voice there        |
| Wow!    | 2.6              | 0.9             | -1.6             | 70             | 60            | 0/3          | 0/3         | Well, ask me anything about voice there     |
| Yep!    | 1.9              | 0.4             | -1.5             | 80             | 90            | 0/3          | 3/3         | Yeah, ask me anything about voice there     |
| Nope!   | 2.3              | 1.0             | -1.3             | 80             | 80            | 0/3          | 3/3         | Noba, ask me anything about voice there     |
| Cool!   | 2.7              | 1.9             | -0.9             | 90             | 110           | 3/3          | 3/3         | Cool, ask me anything about voice there     |
| Done!   | 2.2              | 0.9             | -1.3             | 70             | 70            | 3/3          | 3/3         | Done ask me anything about voice there      |
| Got it! | 2.8              | 0.3             | -2.6             | 80             | 110           | 0/3          | 0/3         | Guarded, ask me anything about voice there. |

## Skipped

- `es` (es, `vits-piper-es-glados-medium`): skipped: English openers do not apply.
- `de` (de, `vits-piper-de_DE-thorsten-medium`): skipped: English openers do not apply.
- `de-thorsten-high` (de, `vits-piper-de_DE-thorsten-high`): skipped: English openers do not apply.
- `de-glados` (de, `vits-piper-de_DE-glados-medium`): skipped: English openers do not apply.
- `fr` (fr, `vits-piper-fr_FR-siwis-medium`): skipped: English openers do not apply.
- `it` (it, `vits-piper-it_IT-paola-medium`): skipped: English openers do not apply.
- `pt` (pt, `vits-piper-pt_BR-faber-medium`): skipped: English openers do not apply.
- `nl` (nl, `vits-piper-nl_NL-pim-medium`): skipped: English openers do not apply.
- `pl` (pl, `vits-piper-pl_PL-gosia-medium`): skipped: English openers do not apply.
- `ru` (ru, `vits-piper-ru_RU-irina-medium`): skipped: English openers do not apply.
- `hi` (hi, `vits-piper-hi_IN-priyamvada-medium`): skipped: English openers do not apply.
- `zh` (zh, `vits-melo-tts-zh_en`): skipped: English openers do not apply.
