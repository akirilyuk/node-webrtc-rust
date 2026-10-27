# Speaker-ID bench

A bench for deciding whether, and how, the voice agent can tell its caller (the main voice) from other people talking nearby. It has two parts:

- `speaker_fixtures` builds the test scenes (`crates/vendor-sherpa-onnx/examples/speaker_fixtures.rs`).
- `speaker_bench` measures speaker-embedding models and the end-to-end effect on transcripts (`crates/vendor-sherpa-onnx/examples/speaker_bench.rs`).

Nothing here changes product behaviour. The models and audio are test and bench assets and are never shipped.

## Fixture design

A **main speaker** says the test phrases. **Other voices** talk in the background, quieter than the main voice, at several levels. The bench measures how much of the main speech is recognized correctly and attributed to the main voice, and how much speech is attributed to the other voices.

- **Main speaker:** `vits-piper-en_US-amy-low`, sid 0. This is the voice the 3000-run phrase qualification used (`examples/voice-agent-local-sherpa/stt-phrase-qualification.json`), so its phrases are valid oracle targets. The main text is every phrase of that file with `below075 == 0` and `keywordMissing` 0 or null.
- **Other voices**, split so a threshold is never tuned on the voices it is tested on:
  - tune set (thresholds are chosen here): `en_US-ryan-medium`, `en_US-kristin-medium`, `en_US-hfc_male-medium`, `en_US-libritts_r-medium` sids 10 and 50;
  - test set (pass rates are reported here): `en_US-joe-medium`, `en_US-hfc_female-medium`, `en_US-lessac-medium`, `en_US-libritts_r-medium` sids 120 and 300.
- **Background text:** 40 short neutral sentences we wrote ourselves (`crates/vendor-sherpa-onnx/examples/support/background_sentences.txt`). None shares a content word (4+ letters) with a main phrase, so leaked background words are detectable in main transcripts. The generator has a test for this.
- **Relative level:** the RMS over voiced 20 ms frames (frame RMS above -45 dBFS) of the other voice divided by that of the main voice: **-6, -10, -15, -20 dB**, plus **0 dB** for a bystander as loud as the caller. The main voice is normalized to -20 dBFS in every scene; if a scene would clip, the whole scene is scaled down and `peak_scale` in the manifest records it.

Scenes (`<out>/scenes/<id>.wav` is 48 kHz stereo s16le with the same audio in both channels, what the agent receives; `<id>.16k.wav` is 16 kHz mono, for embedding and STT):

| Scene                 | Content                                                                                                                                                                                                                                                                                                                 |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| S1 `main_over_babble` | Per main phrase x level (-6/-10/-15/-20) x set: 300 ms lead, the phrase, 1500 ms tail. Babble (two simultaneous other voices reading background sentences with 150-400 ms gaps, level-matched together, then scaled to the level) runs across the whole clip.                                                           |
| S2 `bystander_alone`  | Per set 20 utterances: one other voice says one background sentence at 0 or -10 dB re the main voice, with babble at -20 dB underneath. There is no main speech.                                                                                                                                                        |
| S3 `conversation`     | Per level x set one file of about a minute: two enrollment utterances of the main voice (babble at -20 dB), then 16 turns alternating main (a qualified phrase) and bystander (one other voice, a background sentence, at 0 dB), with 600-900 ms gaps. Babble runs throughout, at the scene level after the enrollment. |

`manifest.json` lists per scene `id`, `kind`, `set`, `level_db`, `babble_db`, `seed`, `peak_scale`, the sha256 of both wav files and `segments: [{start_ms, end_ms, role, voice, sid, text}]` with `role` one of `main`, `bystander`, `babble`. Enrollment utterances carry `"enroll": true`. Every random choice is seeded from 20261027 and the scene id.

`--quick` builds a smoke set: S1 for 3 phrases at -10 dB, S2 with 4 utterances per set, and one S3 at -10 dB (tune set) of 6 turns.

### Determinism

VITS sampling is random, so synthesizing the same text twice gives different audio. The generator therefore keeps every synthesized clip in a cache (default `.models/speaker-tts-cache`, `--cache <dir>` to change it), keyed by bundle, speaker id and text. Running it twice with the same arguments reads the same clips and writes byte-identical files; the sha256 of `manifest.json` (which contains the sha256 of every wav) is printed last. Delete the cache to draw new voices.

## What the bench measures

**Part 1, embedding level**, per embedding model x check length (0.5, 1.0, 1.5 s) x level row:

- _reference_: the embedding of the two enrollment utterances of the main voice in the S3 scene of the same set and level, concatenated;
- _genuine_: the first `check` seconds of every S1 main phrase and S3 main turn; _impostor_: the first `check` seconds of every S2 bystander utterance and S3 bystander turn;
- _score_: cosine similarity of the clip embedding and the reference; a clip is accepted when its score is above the threshold;
- **EER**: the error rate where false accepts (FAR) and false rejects (FRR) are equal;
- **FRR at FAR 1 %** and the **threshold at FAR 1 %**: both from the **tune set** only; then that threshold is applied to the **test set** and its FRR and FAR are reported;
- the row `real voices (clean)` uses the Sherpa real-voice clips (enroll `*-sr-*`, test `*-test-sr-*`); it has no tune/test split;
- `compute` time per clip (stream, waveform, embedding), p50 and p99, on one core.

**Part 2, end to end**, on the S3 scenes of the test set: each scene is fed in 20 ms frames of 48 kHz stereo PCM through the speech crate's `VadEngine` with `VadConfig::default()` (energy VAD). Each VAD utterance is transcribed with the Zipformer online recognizer, embedded (first `check` seconds, with the model and length with the lowest tune-set EER and its tune-set threshold) and attributed to the caller or to someone else. It is matched to the manifest segment it overlaps most, if that is at least 50 % of the utterance. Per level it reports:

- (a) share of main turns whose transcript passes the roundtrip oracle (`wordSimilarity` >= 0.75);
- (b) share of main turns attributed to the caller;
- (c) share of bystander turns attributed to others;
- (d) share of bystander turns attributed to the caller (false accepts);
- (e) background-word leakage: background words (4+ letters, not in a main phrase) found in main-turn transcripts, divided by the words of those transcripts;
- (f) VAD utterances that match no segment, and how they were attributed.

A turn that no VAD utterance matches counts as missed; the counts are in the table. The gaps between turns (600-900 ms) are shorter than the default `min_silence_duration_ms` (1100 ms), so with the default VAD some neighbouring turns merge into one utterance and show up as missed or unmatched. `--vad-min-silence-ms <n>` overrides only that value to show the effect.

Outputs: `results.json` (all numbers plus `nproc`, CPU model, git sha from `NWR_BENCH_GIT_SHA` or `git rev-parse HEAD`, manifest sha256) and `summary.md`.

## Licenses

Speaker-embedding models (release tag `speaker-recongition-models` (sic) of `k2-fsa/sherpa-onnx`). A model whose license does not allow commercial use, or states none, is dropped.

| Model                                               | License                                  | Source                                                                                                                                                                                                                                                                                                                                                                                                                  | Kept |
| --------------------------------------------------- | ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---- |
| `3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx` | Apache-2.0                               | ModelScope card `iic/speech_campplus_sv_en_voxceleb_16k` ("Apache License 2.0", https://modelscope.cn/models/iic/speech_campplus_sv_en_voxceleb_16k); repo license https://github.com/modelscope/3D-Speaker/blob/main/LICENSE                                                                                                                                                                                           | yes  |
| `wespeaker_en_voxceleb_resnet34_LM.onnx`            | CC BY 4.0 (attribution required)         | Hugging Face card `Wespeaker/wespeaker-resnet34-LM` (`license: cc-by-4.0`, https://huggingface.co/Wespeaker/wespeaker-resnet34-LM); WeSpeaker docs ("the pretrained model on VoxCeleb follows Creative Commons Attribution 4.0", https://github.com/wenet-e2e/wespeaker/blob/master/docs/pretrained.md)                                                                                                                 | yes  |
| `nemo_en_titanet_small.onnx`                        | License of the NeMo Toolkit (Apache-2.0) | NGC model card `nvidia/nemo/titanet_small` ("License to use this model is covered by the license of the NeMo Toolkit", https://catalog.ngc.nvidia.com/orgs/nvidia/teams/nemo/models/titanet_small); toolkit license https://github.com/NVIDIA/NeMo/blob/main/LICENSE. The Hugging Face card of the large model says CC BY 4.0; the small model has no Hugging Face card we could open. Both terms allow commercial use. | yes  |

Piper voices (release tag `tts-models`). They are test-only assets, never shipped, but their dataset licenses are recorded. Source for each: `MODEL_CARD` of the voice in https://huggingface.co/rhasspy/piper-voices (the same file is in the extracted bundle).

| Voice                        | Dataset license (as stated on the card)  | Dataset                                                                           |
| ---------------------------- | ---------------------------------------- | --------------------------------------------------------------------------------- |
| `en_US-amy-low` (main voice) | "See URL", no license stated on the card | https://github.com/MycroftAI/mimic3-voices                                        |
| `en_US-ryan-medium`          | CC BY-NC-SA 4.0                          | https://www.kaggle.com/datasets/roholazandie/ryanspeech                           |
| `en_US-kristin-medium`       | public domain                            | https://librivox.org                                                              |
| `en_US-hfc_male-medium`      | CC BY-NC-SA 4.0                          | https://ast-astrec.nict.go.jp/en/release/hi-fi-captain/                           |
| `en_US-libritts_r-medium`    | CC BY 4.0                                | http://www.openslr.org/141/                                                       |
| `en_US-joe-medium`           | CC0                                      | https://github.com/OHF-Voice/voice-datasets                                       |
| `en_US-hfc_female-medium`    | CC BY-NC-SA 4.0                          | https://ast-astrec.nict.go.jp/en/release/hi-fi-captain/                           |
| `en_US-lessac-medium`        | Blizzard 2013 Lessac license             | https://www.cstr.ed.ac.uk/projects/blizzard/2013/lessac_blizzard2013/license.html |

The non-commercial and unstated licenses (ryan, hfc_male, hfc_female, lessac, amy) are acceptable only because the audio is generated for tests and bench runs and is not redistributed in a product, image or package. The test-set `.wav` artifacts of the workflow expire after 14 days.

## Run it locally

Models: `npm run download-tts:en`, `npm run download-stt:en` and `npm run download-speaker` (all with `--workspace=@node-webrtc-rust/example-voice-agent-local-sherpa`). `download-speaker` checks every file against the release's `checksum.txt` and exits 1 naming the file on a mismatch.

Heavy local runs go through the workspace wrapper (it waits for free memory and stops a runaway process):

```bash
cd node-webrtc-rust
M=examples/voice-agent-local-sherpa/.models
H=/Users/akirilyuk/GIT/webrtc-cloud/development/scripts/host-safety/heavy-run.sh
$H --label speaker-fixtures -- cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
  --models $M/speaker --main-tts $M/vits-piper-en_US-amy-low \
  --phrases examples/voice-agent-local-sherpa/stt-phrase-qualification.json --out .test-logs/speaker-fixtures [--quick]
$H --label speaker-bench -- cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_bench -- \
  --fixtures .test-logs/speaker-fixtures --models $M/speaker \
  --stt-model $M/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 --out .test-logs/speaker-bench
```

The other voices' bundles are read from the parent of `--models` (`--voices-root` overrides it).

## Run it on the EU runner

The workflow `.github/workflows/speaker-bench.yml` is dispatch-only. Load work always goes to the EU self-hosted runners, never to the ARC scale sets:

```bash
gh workflow run speaker-bench.yml -R akirilyuk/node-webrtc-rust --ref main -f runner=self-hosted -f quick=false
```

It runs `speaker_bench` under `taskset -c 2`, writes `summary.md` to the step summary, uploads `results.json`, `summary.md` and `manifest.json` as `speaker-bench-<run id>` and the test-set conversation scenes as `speaker-scenes-<run id>` (14 days). The workflow file must be on `main` before it can be dispatched. Check `gh run list -R akirilyuk/node-webrtc-rust --status in_progress` and `--status queued` first: one long run at a time.
