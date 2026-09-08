# Clip encoding fixtures

Stereo sine tones (~1.75 s, amplitude 0.25, 48 kHz) used by `session-pod-clip-playback.integration.test.ts`. Each format uses a distinct frequency so Goertzel assertions can tell encodings apart.

| File             | Format                  | Frequency |
| ---------------- | ----------------------- | --------- |
| `tone-wav.wav`   | WAV PCM                 | 440 Hz    |
| `tone-mp3.mp3`   | MP3 (libmp3lame)        | 523 Hz    |
| `tone-flac.flac` | FLAC                    | 659 Hz    |
| `tone-ogg.ogg`   | Ogg Vorbis              | 784 Hz    |
| `tone-aac.aac`   | AAC ADTS                | 880 Hz    |
| `tone-m4a.m4a`   | AAC in M4A (faststart)  | 988 Hz    |
| `tone-pcm.pcm`   | raw s16le stereo 48 kHz | 1100 Hz   |

Tests load these files via `loadClipFixtures()` — they do **not** run ffmpeg or write to tmpdir.

## Regenerate (manual only)

Requires ffmpeg at `/opt/homebrew/bin/ffmpeg` (or set `FFMPEG`).

```bash
cd packages/helpers/tests/fixtures/clips
node generate-fixtures.mjs
```

The script writes a temporary `*.src.wav` per spec, encodes with the same ffmpeg flags as the original test helper, and overwrites the committed binaries. Review diffs before committing.
